//! What King's answers cost, and what is left: the line under every answer.
//!
//! The person always sees, under what King said: the tokens of that turn
//! (read, of which cached, and written), how fast they were written, what it
//! cost, what was spent today against the budget, and how much of each
//! agent's usage windows is left. Leon writes the line from what the agents
//! and the usage readings say, so it is never a guess of the model's. King
//! reads the same in its instructions before every turn.
//!
//! Pure: numbers in, text out.

use leon_turn::Tokens;
use serde::{Deserialize, Serialize};

/// One turn, measured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnStats {
    /// Read, of which cached.
    pub input: u64,
    /// Of what was read, from the cache.
    pub cached_input: u64,
    /// Written, reasoning included.
    pub output: u64,
    /// How long the turn took, in milliseconds.
    pub duration_ms: u64,
    /// What it cost, when the agent said.
    pub cost_usd: Option<f64>,
}

impl TurnStats {
    /// A turn's numbers from its tokens, how long it took and its cost.
    pub fn of(tokens: &Tokens, duration_ms: u64, cost_usd: Option<f64>) -> Self {
        Self {
            input: tokens.input,
            cached_input: tokens.cached_input,
            output: tokens.output,
            duration_ms,
            cost_usd,
        }
    }

    /// Read and written.
    pub fn total(&self) -> u64 {
        self.input + self.output
    }

    /// Tokens written per second, over the whole turn.
    pub fn per_second(&self) -> Option<f64> {
        (self.duration_ms > 0 && self.output > 0)
            .then(|| self.output as f64 * 1000.0 / self.duration_ms as f64)
    }
}

/// Tokens added up over many turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    /// Read.
    pub input: u64,
    /// Of those, cached.
    pub cached_input: u64,
    /// Written.
    pub output: u64,
}

impl Totals {
    /// Adds a turn.
    pub fn add(&mut self, turn: &TurnStats) {
        self.input += turn.input;
        self.cached_input += turn.cached_input;
        self.output += turn.output;
    }

    /// Read and written.
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}

/// One usage window of an agent, as Leon read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// Which (`5h`, `week`, `month`).
    pub label: String,
    /// The share used, 0 to 1.
    pub used: f64,
    /// Seconds until it resets, when known.
    pub resets_in_secs: Option<i64>,
}

/// An agent's usage windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentWindows {
    /// The agent's tag.
    pub agent: String,
    /// Its windows; none when Leon could not read them.
    pub windows: Vec<Window>,
}

/// What a report line is made of.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts<'a> {
    /// The turn just ended, when it was measured.
    pub turn: Option<TurnStats>,
    /// What the workers used today.
    pub workers_today: Totals,
    /// What King used today.
    pub king_today: Totals,
    /// What was spent today.
    pub spent_today_usd: f64,
    /// The day's budget.
    pub budget_usd: Option<f64>,
    /// The agents' usage windows.
    pub usage: &'a [AgentWindows],
}

/// The facts under one answer, kept with it: what a client draws its own
/// way, and what [`line`] writes as text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The turn that just ended.
    pub turn: Option<TurnStats>,
    /// The workers' tokens today.
    pub workers_today: Totals,
    /// King's own tokens today.
    pub king_today: Totals,
    /// What was spent today, in US dollars.
    pub spent_today_usd: f64,
    /// The day's budget, when there is one.
    pub budget_usd: Option<f64>,
    /// What is left of each agent's usage windows.
    pub usage: Vec<AgentWindows>,
}

impl Report {
    /// The facts, as [`line`] reads them.
    pub fn facts(&self) -> Facts<'_> {
        Facts {
            turn: self.turn,
            workers_today: self.workers_today,
            king_today: self.king_today,
            spent_today_usd: self.spent_today_usd,
            budget_usd: self.budget_usd,
            usage: &self.usage,
        }
    }
}

/// A count of tokens, short: `950`, `12.3k`, `1.2M`.
pub fn count(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

/// A span of seconds, short: `45s`, `12m`, `3h10m`, `2d4h`.
pub fn span(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d{}h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

/// The line under King's answer.
pub fn line(facts: &Facts<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(turn) = facts.turn {
        let mut text = format!(
            "This answer: {} tokens (read {}, {} cached · wrote {})",
            count(turn.total()),
            count(turn.input),
            count(turn.cached_input),
            count(turn.output)
        );
        if let Some(rate) = turn.per_second() {
            text.push_str(&format!(" · {rate:.0} tok/s"));
        }
        if turn.duration_ms > 0 {
            text.push_str(&format!(" · {}", span((turn.duration_ms / 1000) as i64)));
        }
        if let Some(cost) = turn.cost_usd {
            text.push_str(&format!(" · ${cost:.3}"));
        }
        parts.push(text);
    }
    let today_tokens = facts.king_today.total() + facts.workers_today.total();
    let mut today = format!(
        "Today: {} tokens (King {}, workers {}) · ${:.2}",
        count(today_tokens),
        count(facts.king_today.total()),
        count(facts.workers_today.total()),
        facts.spent_today_usd
    );
    if let Some(budget) = facts.budget_usd {
        let left = (budget - facts.spent_today_usd).max(0.0);
        today.push_str(&format!(" of ${budget:.2} (${left:.2} left)"));
    }
    parts.push(today);
    let usage: Vec<String> = facts
        .usage
        .iter()
        .filter(|agent| !agent.windows.is_empty())
        .map(|agent| {
            let windows: Vec<String> = agent
                .windows
                .iter()
                .map(|window| {
                    let left = ((1.0 - window.used) * 100.0).clamp(0.0, 100.0);
                    match window.resets_in_secs {
                        Some(secs) => {
                            format!("{} {left:.0}% left, resets in {}", window.label, span(secs))
                        }
                        None => format!("{} {left:.0}% left", window.label),
                    }
                })
                .collect();
            format!("{}: {}", agent.agent, windows.join(", "))
        })
        .collect();
    if usage.is_empty() {
        parts.push("Usage windows: none read yet (open Leon's usage meters once)".to_owned());
    } else {
        parts.push(format!("Left: {}", usage.join(" · ")));
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_spans_read_short() {
        assert_eq!(count(950), "950");
        assert_eq!(count(12_345), "12.3k");
        assert_eq!(count(1_234_567), "1.2M");
        assert_eq!(span(45), "45s");
        assert_eq!(span(720), "12m");
        assert_eq!(span(11_400), "3h10m");
        assert_eq!(span(187_200), "2d4h");
    }

    #[test]
    fn the_line_says_the_turn_the_day_and_what_is_left() {
        let usage = [
            AgentWindows {
                agent: "claude".into(),
                windows: vec![
                    Window {
                        label: "5h".into(),
                        used: 0.34,
                        resets_in_secs: Some(7800),
                    },
                    Window {
                        label: "week".into(),
                        used: 0.12,
                        resets_in_secs: None,
                    },
                ],
            },
            AgentWindows {
                agent: "codex".into(),
                windows: Vec::new(),
            },
        ];
        let turn = TurnStats {
            input: 45_100,
            cached_input: 31_000,
            output: 3_100,
            duration_ms: 50_000,
            cost_usd: Some(0.042),
        };
        let mut king = Totals::default();
        king.add(&turn);
        let text = line(&Facts {
            turn: Some(turn),
            workers_today: Totals {
                input: 100_000,
                cached_input: 0,
                output: 2_000,
            },
            king_today: king,
            spent_today_usd: 1.27,
            budget_usd: Some(20.0),
            usage: &usage,
        });
        assert_eq!(
            text,
            "This answer: 48.2k tokens (read 45.1k, 31.0k cached · wrote 3.1k) · 62 tok/s · 50s · $0.042\n\
             Today: 150.2k tokens (King 48.2k, workers 102.0k) · $1.27 of $20.00 ($18.73 left)\n\
             Left: claude: 5h 66% left, resets in 2h10m, week 88% left"
        );
        let bare = line(&Facts::default());
        assert!(bare.contains("none read yet"), "{bare}");
        assert!(!bare.contains("This answer"));
    }
}
