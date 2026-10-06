//! What Leon knows about an agent's usage limits.
//!
//! The model is provider neutral: a reading is a list of [`UsageWindow`]s
//! (the five-hour window, the weekly one, a per-model bucket, a month) or an
//! explicit [`Reason`] why nothing is known. Times are Unix seconds and the
//! current time is always passed in, so everything here is a pure function.
//!
//! Staleness is part of the model. A reading describes the moment it was
//! observed; [`AgentUsage::effective`] says what it means *now*: a window
//! whose reset time has passed is reported as reset since it was last seen,
//! never as the old percentage, and a reading older than anything useful
//! becomes unknown.

use leon_core::AgentId;
use serde::{Deserialize, Serialize};

/// Seconds in a minute, an hour and a day.
pub const MINUTE: i64 = 60;
/// Seconds in an hour.
pub const HOUR: i64 = 3600;
/// Seconds in a day.
pub const DAY: i64 = 86_400;

/// Which limit a window is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum WindowKind {
    /// The rolling five-hour window.
    FiveHour,
    /// The weekly window.
    Weekly,
    /// A weekly bucket of one model.
    ModelWeekly(String),
    /// A monthly window.
    Monthly,
    /// Any other limit, with the label the source gave it.
    Custom(String),
}

impl WindowKind {
    /// A short label for a meter: `5h`, `wk`, `mo`, or the model's name.
    pub fn short(&self) -> String {
        match self {
            WindowKind::FiveHour => "5h".into(),
            WindowKind::Weekly => "wk".into(),
            WindowKind::ModelWeekly(model) => model.clone(),
            WindowKind::Monthly => "mo".into(),
            WindowKind::Custom(label) => label.clone(),
        }
    }

    /// A readable name: `5-hour window`, `Weekly`, `Weekly, Fable`.
    pub fn long(&self) -> String {
        match self {
            WindowKind::FiveHour => "5-hour window".into(),
            WindowKind::Weekly => "Weekly".into(),
            WindowKind::ModelWeekly(model) => format!("Weekly, {model}"),
            WindowKind::Monthly => "Monthly".into(),
            WindowKind::Custom(label) => label.clone(),
        }
    }

    /// A stable key for the stored history of this window.
    pub fn key(&self) -> String {
        match self {
            WindowKind::FiveHour => "five_hour".into(),
            WindowKind::Weekly => "weekly".into(),
            WindowKind::ModelWeekly(model) => format!("model_weekly:{model}"),
            WindowKind::Monthly => "monthly".into(),
            WindowKind::Custom(label) => format!("custom:{label}"),
        }
    }
}

/// One limit and how much of it is used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// Which limit.
    pub kind: WindowKind,
    /// How much is used, 0 to 100.
    pub used_percent: f64,
    /// When the window resets (Unix seconds), when the source says.
    pub resets_at: Option<i64>,
    /// How long the window is, in seconds, when the source says.
    pub window_length: Option<i64>,
}

/// Where a reading came from, from the least to the most intrusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A file the agent already writes.
    Local,
    /// The agent's own command line.
    Cli,
    /// The vendor's usage endpoint, called with the agent's own credential.
    VendorApi,
}

impl Source {
    /// A plain description, as shown in the usage view.
    pub fn describe(self, agent: AgentId) -> String {
        match (self, agent) {
            (Source::Local, AgentId::CODEX) => "Codex's own session log".into(),
            (Source::Local, _) => "the agent's own files".into(),
            (Source::Cli, _) => "the agent's command line".into(),
            (Source::VendorApi, AgentId::CLAUDE) => "Anthropic's usage endpoint".into(),
            (Source::VendorApi, AgentId::OPENCODE) => "opencode's usage endpoint".into(),
            (Source::VendorApi, AgentId::CODEX) => "OpenAI's usage endpoint".into(),
            (Source::VendorApi, other) => format!("{}'s usage endpoint", other.name()),
        }
    }
}

/// Why nothing is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The agent is not installed or has never been used here.
    NotInstalled,
    /// The agent has no account signed in.
    NotSignedIn,
    /// The only source is a network call and it is switched off.
    SourceDisabled,
    /// The last observation is too old to say anything.
    DataTooOld,
    /// This machine or this agent has no way to tell.
    NotSupported,
    /// The source answered something that was not understood.
    ParseError,
    /// The source could not be reached or refused.
    Unreachable,
    /// The agent has not written any limit yet.
    NoData,
    /// A read is under way. Never stored: the window shows it while a
    /// collection runs, instead of what the last one said.
    Reading,
    /// The computer is offline, or the vendor did not answer in time.
    Offline,
    /// The vendor answered with an error; this is its HTTP status.
    VendorError(u16),
    /// The vendor asked for fewer calls (HTTP 429); this is the wait it named
    /// in seconds, 0 when it named none.
    RateLimited(u32),
    /// The system refused to hand over the agent's sign-in (a keychain prompt
    /// that was denied or dismissed).
    KeychainDenied,
    /// The agent's stored sign-in has expired. Leon never refreshes it: the
    /// agent does that the next time it runs.
    SessionExpired,
    /// The plan has no limit to measure.
    Unlimited,
}

impl Reason {
    /// One sentence for a tooltip.
    pub fn sentence(self) -> &'static str {
        match self {
            Reason::NotInstalled => "Not installed on this machine.",
            Reason::NotSignedIn => "Not signed in.",
            Reason::SourceDisabled => "The network source is off; turn it on in Settings, Usage.",
            Reason::DataTooOld => "The last reading is too old to be useful.",
            Reason::NotSupported => "Limits cannot be read for this agent here.",
            Reason::ParseError => "The answer was not understood.",
            Reason::Unreachable => "The source could not be reached.",
            Reason::NoData => "No limit has been recorded yet.",
            Reason::Reading => "Reading the limits now.",
            Reason::Offline => "Offline: the service did not answer.",
            Reason::VendorError(_) => "The service answered with an error.",
            Reason::RateLimited(_) => "Rate limited: the service asked for fewer calls.",
            Reason::KeychainDenied => "The sign-in could not be read: access was denied.",
            Reason::SessionExpired => {
                "The sign-in has expired. Run the agent once: it refreshes it by itself."
            }
            Reason::Unlimited => "The plan has no limit to measure.",
        }
    }

    /// The sentence with what it knows, for a line of its own: the vendor's
    /// status is part of it.
    pub fn text(self) -> String {
        match self {
            Reason::VendorError(status) => {
                format!("The service answered with an error (HTTP {status}).")
            }
            Reason::RateLimited(0) => "Rate limited: the service asked for fewer calls.".to_owned(),
            Reason::RateLimited(seconds) => format!(
                "Rate limited: the service asked to wait {}.",
                crate::present::compact_duration(i64::from(seconds))
            ),
            other => other.sentence().to_owned(),
        }
    }

    /// Whether this is a source that failed (as opposed to one that is off,
    /// absent or not applicable): what backs a source off and what a later
    /// reading may retry.
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            Reason::Unreachable
                | Reason::ParseError
                | Reason::Offline
                | Reason::VendorError(_)
                | Reason::RateLimited(_)
                | Reason::KeychainDenied
        )
    }

    /// Two or three words for the bar.
    pub fn short(self) -> &'static str {
        match self {
            Reason::NotInstalled => "not installed",
            Reason::NotSignedIn => "signed out",
            Reason::SourceDisabled => "source off",
            Reason::DataTooOld => "too old",
            Reason::NotSupported => "unsupported",
            Reason::ParseError => "unreadable",
            Reason::Unreachable => "unreachable",
            Reason::NoData => "no data yet",
            Reason::Reading => "reading…",
            Reason::Offline => "offline",
            Reason::VendorError(_) => "error",
            Reason::RateLimited(_) => "rate limited",
            Reason::KeychainDenied => "access denied",
            Reason::SessionExpired => "sign-in expired",
            Reason::Unlimited => "no limit",
        }
    }
}

/// What a reading holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// The limits seen.
    Known {
        /// The windows, in the source's order.
        windows: Vec<UsageWindow>,
    },
    /// Nothing is known, and why.
    Unknown {
        /// Why.
        reason: Reason,
    },
}

/// One observation of the limits of one agent on one machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentUsage {
    /// Which agent.
    pub agent: AgentId,
    /// The machine's id.
    pub machine: String,
    /// A label for the account that never identifies it: the plan or nothing.
    pub account_label: Option<String>,
    /// The plan, when known (`plus`, `max`).
    pub plan: Option<String>,
    /// Where the numbers came from; absent when nothing was read.
    pub source: Option<Source>,
    /// When it was observed (Unix seconds); absent when nothing was read.
    pub observed_at: Option<i64>,
    /// The windows or the reason.
    pub state: State,
}

/// A reading together with the observations that came with it, which feed the
/// stored history and the burn-rate forecast.
#[derive(Clone, Debug, PartialEq)]
pub struct Collected {
    /// The latest reading.
    pub usage: AgentUsage,
    /// Earlier and current observations of its windows.
    pub samples: Vec<(WindowKind, crate::forecast::Sample)>,
}

/// What one window means at a given time.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveWindow {
    /// Which limit.
    pub kind: WindowKind,
    /// What to show: 0 when the window has reset since it was seen.
    pub used_percent: f64,
    /// When it resets, while that is still ahead.
    pub resets_at: Option<i64>,
    /// How long the window is.
    pub window_length: Option<i64>,
    /// Whether the window reset after the reading was taken.
    pub reset_since_seen: bool,
}

/// What a reading means at a given time.
#[derive(Clone, Debug, PartialEq)]
pub enum Effective {
    /// The windows as of now.
    Windows(Vec<EffectiveWindow>),
    /// Nothing usable, and why.
    Unknown(Reason),
}

/// How old a reading may be before it says nothing, when none of its windows
/// gives a reset time to judge by.
pub const MAX_AGE_WITHOUT_RESET: i64 = 2 * HOUR;
/// How old a reading may be before it says nothing, whatever it holds.
pub const MAX_AGE: i64 = 8 * DAY;

impl AgentUsage {
    /// A reading with nothing known.
    pub fn unknown(agent: AgentId, machine: &str, reason: Reason) -> Self {
        Self {
            agent,
            machine: machine.to_owned(),
            account_label: None,
            plan: None,
            source: None,
            observed_at: None,
            state: State::Unknown { reason },
        }
    }

    /// What the reading means at `now`.
    ///
    /// A window whose reset time is not after `now` has reset since the
    /// reading was taken and is shown at 0 %, flagged. A window with no reset
    /// time is trusted for [`MAX_AGE_WITHOUT_RESET`] only. A reading older
    /// than [`MAX_AGE`] is unknown.
    pub fn effective(&self, now: i64) -> Effective {
        let windows = match &self.state {
            State::Unknown { reason } => return Effective::Unknown(*reason),
            State::Known { windows } => windows,
        };
        let observed = self.observed_at.unwrap_or(now);
        let age = (now - observed).max(0);
        if age > MAX_AGE {
            return Effective::Unknown(Reason::DataTooOld);
        }
        let mut out = Vec::with_capacity(windows.len());
        for window in windows {
            match window.resets_at {
                Some(reset) if reset <= now => out.push(EffectiveWindow {
                    kind: window.kind.clone(),
                    used_percent: 0.0,
                    resets_at: None,
                    window_length: window.window_length,
                    reset_since_seen: true,
                }),
                Some(_) => out.push(keep(window)),
                None if age <= MAX_AGE_WITHOUT_RESET => out.push(keep(window)),
                None => {}
            }
        }
        if out.is_empty() {
            return Effective::Unknown(Reason::DataTooOld);
        }
        Effective::Windows(out)
    }
}

fn keep(window: &UsageWindow) -> EffectiveWindow {
    EffectiveWindow {
        kind: window.kind.clone(),
        used_percent: window.used_percent.clamp(0.0, 100.0),
        resets_at: window.resets_at,
        window_length: window.window_length,
        reset_since_seen: false,
    }
}

impl Effective {
    /// The window closest to its limit, the one a compact view shows.
    pub fn primary(&self) -> Option<&EffectiveWindow> {
        match self {
            Effective::Windows(windows) => windows
                .iter()
                .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent)),
            Effective::Unknown(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    fn window(kind: WindowKind, used: f64, resets_at: Option<i64>) -> UsageWindow {
        UsageWindow {
            kind,
            used_percent: used,
            resets_at,
            window_length: None,
        }
    }

    fn reading(observed_at: i64, windows: Vec<UsageWindow>) -> AgentUsage {
        AgentUsage {
            agent: AgentId::CODEX,
            machine: "local".into(),
            account_label: None,
            plan: Some("plus".into()),
            source: Some(Source::Local),
            observed_at: Some(observed_at),
            state: State::Known { windows },
        }
    }

    #[test]
    fn a_window_that_has_not_reset_keeps_its_percentage() {
        let usage = reading(
            NOW - 600,
            vec![window(WindowKind::FiveHour, 38.0, Some(NOW + 3600))],
        );
        let Effective::Windows(windows) = usage.effective(NOW) else {
            panic!("expected windows");
        };
        assert_eq!(windows[0].used_percent, 38.0);
        assert!(!windows[0].reset_since_seen);
    }

    #[test]
    fn a_window_that_reset_since_it_was_seen_reads_zero_and_says_so() {
        let usage = reading(
            NOW - DAY,
            vec![window(WindowKind::FiveHour, 97.0, Some(NOW - HOUR))],
        );
        let Effective::Windows(windows) = usage.effective(NOW) else {
            panic!("expected windows");
        };
        assert_eq!(windows[0].used_percent, 0.0);
        assert!(windows[0].reset_since_seen);
        assert_eq!(windows[0].resets_at, None);
    }

    #[test]
    fn a_window_resetting_exactly_now_has_reset() {
        let usage = reading(NOW - 60, vec![window(WindowKind::Weekly, 50.0, Some(NOW))]);
        let Effective::Windows(windows) = usage.effective(NOW) else {
            panic!("expected windows");
        };
        assert!(windows[0].reset_since_seen);
    }

    #[test]
    fn a_window_without_a_reset_time_is_trusted_only_for_a_short_while() {
        let fresh = reading(NOW - HOUR, vec![window(WindowKind::Monthly, 10.0, None)]);
        assert!(matches!(fresh.effective(NOW), Effective::Windows(_)));
        let old = reading(
            NOW - 3 * HOUR,
            vec![window(WindowKind::Monthly, 10.0, None)],
        );
        assert_eq!(old.effective(NOW), Effective::Unknown(Reason::DataTooOld));
    }

    #[test]
    fn a_reading_older_than_the_longest_window_is_unknown() {
        let usage = reading(
            NOW - 9 * DAY,
            vec![window(WindowKind::Weekly, 40.0, Some(NOW + DAY))],
        );
        assert_eq!(usage.effective(NOW), Effective::Unknown(Reason::DataTooOld));
    }

    #[test]
    fn an_unknown_reading_keeps_its_reason() {
        let usage = AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::SourceDisabled);
        assert_eq!(
            usage.effective(NOW),
            Effective::Unknown(Reason::SourceDisabled)
        );
    }

    #[test]
    fn the_primary_window_is_the_one_closest_to_its_limit() {
        let usage = reading(
            NOW,
            vec![
                window(WindowKind::FiveHour, 10.0, Some(NOW + 100)),
                window(WindowKind::Weekly, 91.0, Some(NOW + 100)),
                window(
                    WindowKind::ModelWeekly("Fable".into()),
                    0.0,
                    Some(NOW + 100),
                ),
            ],
        );
        let effective = usage.effective(NOW);
        assert_eq!(effective.primary().unwrap().kind, WindowKind::Weekly);
    }

    #[test]
    fn percentages_outside_the_range_are_clamped() {
        let usage = reading(
            NOW,
            vec![window(WindowKind::FiveHour, 140.0, Some(NOW + 100))],
        );
        assert_eq!(usage.effective(NOW).primary().unwrap().used_percent, 100.0);
    }

    #[test]
    fn labels_are_short_for_meters_and_long_for_the_popover() {
        assert_eq!(WindowKind::FiveHour.short(), "5h");
        assert_eq!(WindowKind::Weekly.short(), "wk");
        assert_eq!(WindowKind::ModelWeekly("Fable".into()).short(), "Fable");
        assert_eq!(
            WindowKind::ModelWeekly("Fable".into()).long(),
            "Weekly, Fable"
        );
        assert_eq!(WindowKind::Monthly.key(), "monthly");
    }

    #[test]
    fn a_reading_survives_a_round_trip_through_json() {
        let usage = reading(
            NOW,
            vec![window(
                WindowKind::ModelWeekly("Fable".into()),
                3.0,
                Some(NOW + 5),
            )],
        );
        let json = serde_json::to_string(&usage).unwrap();
        assert_eq!(serde_json::from_str::<AgentUsage>(&json).unwrap(), usage);
    }
}
