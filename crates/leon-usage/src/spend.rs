//! Tokens and their estimated cost, summed for the usage view.
//!
//! The store keeps counts per agent, machine, model and UTC day;
//! [`report`] turns those rows into what the view shows: the total, the cache
//! share, and the same sums per agent, per model and per day, each with an
//! **estimated API-equivalent cost** from a [`PriceTable`]. A model without a
//! price keeps its tokens and has no cost: a sum that mixes both says how many
//! of its tokens were left out ([`Cost::unpriced`]) instead of pretending to
//! be complete.
//!
//! Everything is a pure function of the rows, the prices and an injected
//! clock; nothing reads the store or draws.

use std::collections::HashMap;

use chrono::{DateTime, Days, NaiveDate};
use leon_core::{AgentId, MachineId, TokenCounts, TokenRow};

use crate::pricing::{normalize_model, PriceTable};

/// The most days the per-day list holds.
pub const DAY_ROWS: usize = 14;

/// The span of days a report covers, ending today (UTC).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Period {
    /// Today and the six days before.
    Week,
    /// The last thirty days.
    #[default]
    Month,
    /// The last ninety days.
    Quarter,
    /// Everything imported.
    All,
}

impl Period {
    /// The next one in the cycle of the view's key.
    pub fn next(self) -> Self {
        match self {
            Self::Week => Self::Month,
            Self::Month => Self::Quarter,
            Self::Quarter => Self::All,
            Self::All => Self::Week,
        }
    }

    /// What the view calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Week => "last 7 days",
            Self::Month => "last 30 days",
            Self::Quarter => "last 90 days",
            Self::All => "all time",
        }
    }

    fn days(self) -> Option<u64> {
        match self {
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::Quarter => Some(90),
            Self::All => None,
        }
    }

    /// The first UTC day (`YYYY-MM-DD`) the period covers when it is `now`
    /// (Unix seconds); `None` for all time.
    pub fn since_day(self, now: i64) -> Option<String> {
        let days = self.days()?;
        let today = DateTime::from_timestamp(now, 0)?.date_naive();
        let first = today
            .checked_sub_days(Days::new(days - 1))
            .unwrap_or(NaiveDate::MIN);
        Some(first.to_string())
    }
}

/// An estimated cost in US dollars, and how much of the tokens it could not
/// cover.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    /// The estimate for the tokens that have a price.
    pub usd: f64,
    /// How many tokens had a price.
    pub priced: u64,
    /// How many tokens had none and are not in `usd`.
    pub unpriced: u64,
}

impl Cost {
    fn add(&mut self, other: &Self) {
        self.usd += other.usd;
        self.priced = self.priced.saturating_add(other.priced);
        self.unpriced = self.unpriced.saturating_add(other.unpriced);
    }

    /// Whether every token had a price.
    pub fn is_complete(&self) -> bool {
        self.unpriced == 0
    }

    /// What a row says: `$1.23`, `$1.23 + unpriced`, `no price`, or `-` when
    /// there is nothing to price.
    pub fn text(&self) -> String {
        match (self.priced, self.unpriced) {
            (0, 0) => "-".to_owned(),
            (0, _) => "no price".to_owned(),
            (_, 0) => usd_text(self.usd),
            _ => format!("{} + unpriced", usd_text(self.usd)),
        }
    }
}

/// One line of a list: a key, its tokens and its cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Line<K> {
    /// The agent, model or day.
    pub key: K,
    /// The tokens.
    pub counts: TokenCounts,
    /// The estimated cost.
    pub cost: Cost,
}

impl<K> Line<K> {
    /// Cache reads over all input, when there is any input.
    pub fn cache_share(&self) -> Option<f64> {
        let input = self.counts.all_input();
        (input > 0).then(|| self.counts.cache_read as f64 / input as f64)
    }

    fn put(&mut self, counts: &TokenCounts, cost: &Cost) {
        self.counts.add(counts);
        self.cost.add(cost);
    }
}

/// What the usage view shows of the tokens.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// The span.
    pub period: Period,
    /// All tokens in the span.
    pub total: Line<()>,
    /// Cache reads over all input, when there is any input.
    pub cache_share: Option<f64>,
    /// Per agent, most tokens first.
    pub by_agent: Vec<Line<AgentId>>,
    /// Per model, most tokens first. The key is the model as the transcript
    /// names it (without a context marker); an empty name is "unknown".
    pub by_model: Vec<Line<String>>,
    /// Per UTC day, newest first, at most [`DAY_ROWS`].
    pub by_day: Vec<Line<String>>,
    /// Whether any token had no price.
    pub partial: bool,
}

impl Report {
    /// Whether nothing was counted.
    pub fn is_empty(&self) -> bool {
        self.total.counts.is_empty()
    }
}

/// The cost of `counts` of `model`.
fn cost_of(table: &PriceTable, model: &str, counts: &TokenCounts) -> Cost {
    let tokens = counts.total();
    match table.cost(model, counts) {
        Some(usd) => Cost {
            usd,
            priced: tokens,
            unpriced: 0,
        },
        None => Cost {
            usd: 0.0,
            priced: 0,
            unpriced: tokens,
        },
    }
}

/// Sums `rows` for the view: only `machine`'s when given, only the days of
/// `period` counted back from `now` (Unix seconds).
pub fn report(
    rows: &[TokenRow],
    table: &PriceTable,
    machine: Option<&MachineId>,
    period: Period,
    now: i64,
) -> Report {
    let since = period.since_day(now);
    let mut total = Line {
        key: (),
        counts: TokenCounts::default(),
        cost: Cost::default(),
    };
    let mut agents: HashMap<AgentId, Line<AgentId>> = HashMap::new();
    let mut models: HashMap<String, Line<String>> = HashMap::new();
    let mut days: HashMap<String, Line<String>> = HashMap::new();

    for row in rows {
        if machine.is_some_and(|wanted| *wanted != row.machine)
            || since
                .as_deref()
                .is_some_and(|first| row.day.as_str() < first)
        {
            continue;
        }
        let cost = cost_of(table, &row.model, &row.counts);
        total.put(&row.counts, &cost);
        agents
            .entry(row.agent)
            .or_insert_with(|| Line {
                key: row.agent,
                counts: TokenCounts::default(),
                cost: Cost::default(),
            })
            .put(&row.counts, &cost);
        days.entry(row.day.clone())
            .or_insert_with(|| Line {
                key: row.day.clone(),
                counts: TokenCounts::default(),
                cost: Cost::default(),
            })
            .put(&row.counts, &cost);
        models
            .entry(normalize_model(&row.model))
            .or_insert_with(|| Line {
                key: model_name(&row.model),
                counts: TokenCounts::default(),
                cost: Cost::default(),
            })
            .put(&row.counts, &cost);
    }

    let heaviest = |a: &TokenCounts, b: &TokenCounts| b.total().cmp(&a.total());
    let mut by_agent: Vec<_> = agents.into_values().collect();
    by_agent.sort_by(|a, b| {
        heaviest(&a.counts, &b.counts).then_with(|| a.key.as_str().cmp(b.key.as_str()))
    });
    let mut by_model: Vec<_> = models.into_values().collect();
    by_model.sort_by(|a, b| heaviest(&a.counts, &b.counts).then_with(|| a.key.cmp(&b.key)));
    let mut by_day: Vec<_> = days.into_values().collect();
    by_day.sort_by(|a, b| b.key.cmp(&a.key));
    by_day.truncate(DAY_ROWS);

    Report {
        period,
        cache_share: total.cache_share(),
        partial: !total.cost.is_complete(),
        total,
        by_agent,
        by_model,
        by_day,
    }
}

/// A model as the list names it: the transcript's own spelling without a
/// context marker; "unknown" when the transcript did not say.
fn model_name(model: &str) -> String {
    let name = model.split('[').next().unwrap_or("").trim();
    if name.is_empty() {
        "unknown".to_owned()
    } else {
        name.to_owned()
    }
}

/// A count of tokens in a few characters: `812`, `1.2k`, `34.5M`, `1.2B`.
pub fn tokens_text(tokens: u64) -> String {
    let figure = |value: f64, unit: &str| {
        if value < 10.0 {
            format!("{value:.2}{unit}")
        } else if value < 100.0 {
            format!("{value:.1}{unit}")
        } else {
            format!("{value:.0}{unit}")
        }
    };
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => figure(tokens as f64 / 1e3, "k"),
        1_000_000..=999_999_999 => figure(tokens as f64 / 1e6, "M"),
        _ => figure(tokens as f64 / 1e9, "B"),
    }
}

/// A dollar amount: `$0.00`, `<$0.01`, `$12.34`, `$1234`.
pub fn usd_text(usd: f64) -> String {
    if usd <= 0.0 {
        "$0.00".to_owned()
    } else if usd < 0.01 {
        "<$0.01".to_owned()
    } else if usd < 1000.0 {
        format!("${usd:.2}")
    } else {
        format!("${usd:.0}")
    }
}

/// A share as a whole percentage: `84%`.
pub fn share_text(share: f64) -> String {
    format!("{:.0}%", (share * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = r#"{"models": [
        {"id": "model-a", "input": 5, "output": 25, "cache_read": 0.5, "cache_write": 6.25,
         "cache_write_1h": 10, "source": "https://example.test", "recorded": "2026-01-01"}
    ]}"#;

    fn table() -> PriceTable {
        PriceTable::empty().with_overrides(TABLE).unwrap().0
    }

    fn row(
        agent: AgentId,
        machine: &str,
        model: &str,
        day: &str,
        input: u64,
        read: u64,
    ) -> TokenRow {
        TokenRow {
            agent,
            machine: MachineId::from_string(machine),
            model: model.to_owned(),
            day: day.to_owned(),
            counts: TokenCounts {
                input,
                output: 1_000_000,
                cache_read: read,
                ..Default::default()
            },
        }
    }

    /// 2026-03-10 12:00 UTC.
    const NOW: i64 = 1_773_144_000;

    fn rows() -> Vec<TokenRow> {
        vec![
            row(
                AgentId::CLAUDE,
                "local",
                "model-a",
                "2026-03-10",
                1_000_000,
                3_000_000,
            ),
            row(
                AgentId::CLAUDE,
                "local",
                "model-a[1m]",
                "2026-03-09",
                1_000_000,
                0,
            ),
            row(
                AgentId::CODEX,
                "local",
                "mystery",
                "2026-03-10",
                500_000,
                500_000,
            ),
            row(
                AgentId::CLAUDE,
                "box",
                "model-a",
                "2026-03-10",
                1_000_000,
                0,
            ),
            row(
                AgentId::CLAUDE,
                "local",
                "model-a",
                "2026-01-01",
                1_000_000,
                0,
            ),
        ]
    }

    #[test]
    fn the_period_counts_back_from_today_in_utc() {
        assert_eq!(Period::Week.since_day(NOW).as_deref(), Some("2026-03-04"));
        assert_eq!(Period::Month.since_day(NOW).as_deref(), Some("2026-02-09"));
        assert_eq!(
            Period::Quarter.since_day(NOW).as_deref(),
            Some("2025-12-11")
        );
        assert_eq!(Period::All.since_day(NOW), None);
        // The last second of the day is still that day.
        assert_eq!(
            Period::Week.since_day(1_773_187_199).as_deref(),
            Some("2026-03-04")
        );
        assert_eq!(
            Period::Week.since_day(1_773_187_200).as_deref(),
            Some("2026-03-05")
        );
        assert_eq!(Period::All.next(), Period::Week);
    }

    #[test]
    fn totals_agents_models_and_days_add_up_and_the_period_and_machine_filter_them() {
        let all = report(&rows(), &table(), None, Period::Month, NOW);
        // 01-01 is outside the month; the other four rows count.
        assert_eq!(
            all.total.counts.input,
            1_000_000 + 1_000_000 + 500_000 + 1_000_000
        );
        assert_eq!(all.by_agent.len(), 2);
        assert_eq!(all.by_agent[0].key, AgentId::CLAUDE, "most tokens first");
        // `model-a[1m]` and `model-a` are one model.
        assert_eq!(all.by_model.len(), 2);
        assert_eq!(all.by_model[0].key, "model-a");
        assert_eq!(all.by_model[0].counts.input, 3_000_000);
        assert_eq!(
            all.by_day
                .iter()
                .map(|d| d.key.as_str())
                .collect::<Vec<_>>(),
            ["2026-03-10", "2026-03-09"]
        );

        let local = report(
            &rows(),
            &table(),
            Some(&MachineId::from_string("local")),
            Period::Month,
            NOW,
        );
        assert_eq!(local.total.counts.input, 2_500_000);
        let ever = report(
            &rows(),
            &table(),
            Some(&MachineId::from_string("local")),
            Period::All,
            NOW,
        );
        assert_eq!(ever.total.counts.input, 3_500_000);
        let week = report(&rows(), &table(), None, Period::Week, NOW);
        assert_eq!(week.total.counts, all.total.counts);
    }

    #[test]
    fn the_cache_share_is_cache_reads_over_all_input() {
        let one = vec![row(
            AgentId::CLAUDE,
            "local",
            "model-a",
            "2026-03-10",
            1_000_000,
            3_000_000,
        )];
        let got = report(&one, &table(), None, Period::Month, NOW);
        assert_eq!(got.cache_share, Some(0.75));
        assert_eq!(share_text(0.75), "75%");
        let none = vec![TokenRow {
            counts: TokenCounts {
                output: 5,
                ..Default::default()
            },
            ..one[0].clone()
        }];
        assert_eq!(
            report(&none, &table(), None, Period::Month, NOW).cache_share,
            None
        );
    }

    #[test]
    fn the_cost_is_priced_per_model_and_an_unknown_model_keeps_its_tokens_without_one() {
        let got = report(&rows(), &table(), None, Period::Month, NOW);
        let model_a = got.by_model.iter().find(|m| m.key == "model-a").unwrap();
        // Three rows: input 3M at 5, output 3M at 25, cache read 3M at 0.5.
        assert!((model_a.cost.usd - (15.0 + 75.0 + 1.5)).abs() < 1e-9);
        assert!(model_a.cost.is_complete());
        let mystery = got.by_model.iter().find(|m| m.key == "mystery").unwrap();
        assert_eq!(mystery.cost.text(), "no price");
        assert_eq!(mystery.counts.total(), 500_000 + 1_000_000 + 500_000);
        assert!(got.partial);
        // The agent that used both says so.
        let codex = got
            .by_agent
            .iter()
            .find(|a| a.key == AgentId::CODEX)
            .unwrap();
        assert_eq!(codex.cost.text(), "no price");
        let only_a = report(&rows()[..2], &table(), None, Period::Month, NOW);
        assert!(!only_a.partial);
        assert!(only_a.total.cost.text().starts_with('$'));
    }

    #[test]
    fn a_sum_with_both_priced_and_unpriced_tokens_says_so() {
        let mixed = Cost {
            usd: 1.5,
            priced: 10,
            unpriced: 5,
        };
        assert_eq!(mixed.text(), "$1.50 + unpriced");
        assert_eq!(Cost::default().text(), "-");
        assert!(!mixed.is_complete());
    }

    #[test]
    fn nothing_counted_is_an_empty_report_and_an_empty_model_name_is_unknown() {
        let empty = report(&[], &table(), None, Period::Month, NOW);
        assert!(empty.is_empty());
        assert!(empty.by_agent.is_empty() && empty.by_day.is_empty());
        let unnamed = vec![row(AgentId::CLAUDE, "local", "", "2026-03-10", 1, 0)];
        let got = report(&unnamed, &table(), None, Period::Month, NOW);
        assert_eq!(got.by_model[0].key, "unknown");
        assert_eq!(got.by_model[0].cost.text(), "no price");
    }

    #[test]
    fn the_day_list_is_newest_first_and_bounded() {
        let many: Vec<_> = (1..=28)
            .map(|day| {
                row(
                    AgentId::CLAUDE,
                    "local",
                    "model-a",
                    &format!("2026-02-{day:02}"),
                    1,
                    0,
                )
            })
            .collect();
        let got = report(&many, &table(), None, Period::All, NOW);
        assert_eq!(got.by_day.len(), DAY_ROWS);
        assert_eq!(got.by_day[0].key, "2026-02-28");
    }

    #[test]
    fn figures_are_short_and_honest_at_the_edges() {
        assert_eq!(tokens_text(0), "0");
        assert_eq!(tokens_text(999), "999");
        assert_eq!(tokens_text(1_000), "1.00k");
        assert_eq!(tokens_text(12_345), "12.3k");
        assert_eq!(tokens_text(345_678), "346k");
        assert_eq!(tokens_text(34_500_000), "34.5M");
        assert_eq!(tokens_text(1_200_000_000), "1.20B");
        assert_eq!(usd_text(0.0), "$0.00");
        assert_eq!(usd_text(0.004), "<$0.01");
        assert_eq!(usd_text(12.346), "$12.35");
        assert_eq!(usd_text(1234.5), "$1234");
    }
}
