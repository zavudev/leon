//! A reading as the bar and the usage view show it.
//!
//! [`view`] turns a reading and the time into what is drawn: one primary meter
//! (the window closest to its limit), the other meters, the plan, where the
//! numbers came from and how old they are, or the reason nothing is known.
//! All wording that is not a layout decision lives here, so the window, the
//! diagnostic command and the tests read the same sentences.

use leon_core::AgentId;

use crate::model::{AgentUsage, Effective, EffectiveWindow, Reason, Source, WindowKind};
use crate::present::{ago, countdown, Level, Thresholds};

/// One window, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    /// Which limit.
    pub kind: WindowKind,
    /// How much is used, 0 to 100.
    pub percent: f64,
    /// How close to the limit.
    pub level: Level,
    /// Seconds until it resets, while that is ahead.
    pub resets_in: Option<i64>,
    /// Whether it reset after the reading was taken.
    pub reset_since_seen: bool,
}

impl Meter {
    fn of(window: &EffectiveWindow, now: i64, thresholds: Thresholds) -> Self {
        Self {
            kind: window.kind.clone(),
            percent: window.used_percent,
            level: thresholds.classify(window.used_percent),
            resets_in: window.resets_at.map(|r| (r - now).max(0)),
            reset_since_seen: window.reset_since_seen,
        }
    }

    /// `5h  38%`, `wk 91% !`, with the marker for the level.
    pub fn text(&self) -> String {
        let marker = self.level.glyph();
        let tail = if marker.is_empty() {
            String::new()
        } else {
            format!(" {marker}")
        };
        format!(
            "{} {}{}",
            self.kind.short(),
            crate::present::percent_fixed(self.percent),
            tail
        )
    }

    /// The same for a figure that says what is left: `5h  62% left`.
    pub fn text_for(&self, display: crate::present::PercentDisplay) -> String {
        let marker = self.level.glyph();
        let tail = if marker.is_empty() {
            String::new()
        } else {
            format!(" {marker}")
        };
        let left = if display == crate::present::PercentDisplay::Remaining {
            " left"
        } else {
            ""
        };
        format!(
            "{} {}{}{}",
            self.kind.short(),
            display.fixed(self.percent),
            left,
            tail
        )
    }

    /// `Resets in 2h 29m`, `Reset since last seen`, or nothing.
    pub fn reset_text(&self) -> Option<String> {
        if self.reset_since_seen {
            return Some("Reset since last seen".into());
        }
        self.resets_in.map(|seconds| match countdown(seconds) {
            text if text == "now" => "Resets now".to_owned(),
            text => format!("Resets in {text}"),
        })
    }
}

/// What a reading comes to.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Limits are known.
    Ready {
        /// The window closest to its limit.
        primary: Meter,
        /// All the windows, the primary one included, in the source's order.
        meters: Vec<Meter>,
    },
    /// Nothing is known.
    Unknown(Reason),
}

/// A reading ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentView {
    /// The agent.
    pub agent: AgentId,
    /// The machine's id.
    pub machine: String,
    /// The plan, when known.
    pub plan: Option<String>,
    /// Where the numbers came from.
    pub source: Option<Source>,
    /// Seconds since they were observed.
    pub age: Option<i64>,
    /// The meters or the reason.
    pub body: Body,
}

impl AgentView {
    /// The worst level among its meters; normal when unknown.
    pub fn level(&self) -> Level {
        match &self.body {
            Body::Ready { primary, .. } => primary.level,
            Body::Unknown(_) => Level::Normal,
        }
    }

    /// `from Codex's own session log, 3 min ago`, or the reason's sentence.
    pub fn provenance(&self) -> String {
        match (&self.body, self.source, self.age) {
            (Body::Unknown(reason), ..) => reason.text(),
            (_, Some(source), Some(age)) => {
                format!("from {}, {}", source.describe(self.agent), ago(age))
            }
            (_, Some(source), None) => format!("from {}", source.describe(self.agent)),
            _ => String::new(),
        }
    }
}

/// The reading `usage` at time `now`.
pub fn view(usage: &AgentUsage, now: i64, thresholds: Thresholds) -> AgentView {
    let effective = usage.effective(now);
    let body = match &effective {
        Effective::Unknown(reason) => Body::Unknown(*reason),
        Effective::Windows(windows) => {
            let meters: Vec<Meter> = windows
                .iter()
                .map(|w| Meter::of(w, now, thresholds))
                .collect();
            let primary = effective
                .primary()
                .map(|w| Meter::of(w, now, thresholds))
                .expect("a reading with windows has a primary one");
            Body::Ready { primary, meters }
        }
    };
    AgentView {
        agent: usage.agent,
        machine: usage.machine.clone(),
        plan: usage.plan.clone(),
        source: usage.source,
        age: usage.observed_at.map(|at| (now - at).max(0)),
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{State, UsageWindow, HOUR};

    const NOW: i64 = 1_790_000_000;

    fn usage(windows: Vec<UsageWindow>) -> AgentUsage {
        AgentUsage {
            agent: AgentId::CODEX,
            machine: "local".into(),
            account_label: None,
            plan: Some("plus".into()),
            source: Some(Source::Local),
            observed_at: Some(NOW - 180),
            state: State::Known { windows },
        }
    }

    fn w(kind: WindowKind, used: f64, resets_in: i64) -> UsageWindow {
        UsageWindow {
            kind,
            used_percent: used,
            resets_at: Some(NOW + resets_in),
            window_length: None,
        }
    }

    #[test]
    fn the_primary_meter_is_the_closest_to_its_limit_with_its_level() {
        let v = view(
            &usage(vec![
                w(WindowKind::FiveHour, 10.0, 2 * HOUR + 29 * 60),
                w(WindowKind::Weekly, 91.0, 30 * HOUR),
            ]),
            NOW,
            Thresholds::default(),
        );
        let Body::Ready { primary, meters } = &v.body else {
            panic!("expected meters");
        };
        assert_eq!(primary.kind, WindowKind::Weekly);
        assert_eq!(primary.level, Level::Critical);
        assert_eq!(meters.len(), 2);
        assert_eq!(meters[0].level, Level::Normal);
        assert_eq!(meters[0].reset_text().as_deref(), Some("Resets in 2h 29m"));
        assert_eq!(v.level(), Level::Critical);
    }

    #[test]
    fn a_meter_says_its_level_in_words_as_well_as_colour() {
        let v = view(
            &usage(vec![w(WindowKind::Weekly, 91.0, HOUR)]),
            NOW,
            Thresholds::default(),
        );
        let Body::Ready { primary, .. } = v.body else {
            panic!()
        };
        assert_eq!(primary.text(), "wk  91% !!");
        let v = view(
            &usage(vec![w(WindowKind::FiveHour, 7.0, HOUR)]),
            NOW,
            Thresholds::default(),
        );
        let Body::Ready { primary, .. } = v.body else {
            panic!()
        };
        assert_eq!(primary.text(), "5h   7%");
    }

    #[test]
    fn a_reset_since_seen_says_so_instead_of_a_countdown() {
        let v = view(
            &usage(vec![w(WindowKind::FiveHour, 97.0, -HOUR)]),
            NOW,
            Thresholds::default(),
        );
        let Body::Ready { primary, .. } = v.body else {
            panic!()
        };
        assert_eq!(primary.percent, 0.0);
        assert_eq!(
            primary.reset_text().as_deref(),
            Some("Reset since last seen")
        );
    }

    #[test]
    fn the_provenance_says_where_and_how_fresh() {
        let v = view(
            &usage(vec![w(WindowKind::FiveHour, 5.0, HOUR)]),
            NOW,
            Thresholds::default(),
        );
        assert_eq!(v.provenance(), "from Codex's own session log, 3 min ago");
    }

    #[test]
    fn an_unknown_reading_gives_its_reason() {
        let v = view(
            &AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::SourceDisabled),
            NOW,
            Thresholds::default(),
        );
        assert_eq!(v.body, Body::Unknown(Reason::SourceDisabled));
        assert_eq!(v.provenance(), Reason::SourceDisabled.sentence());
        assert_eq!(v.level(), Level::Normal);
    }
}
