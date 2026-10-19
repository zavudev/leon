//! What King knows of the models it can give work to: their talents and
//! their benchmarks.
//!
//! Which models there are comes from the agents themselves: each one is
//! asked what it offers (`leon_turn::Adapter::options`), so the list is the
//! real one of this computer. What each model is good at is not something an
//! agent says. That is a [`Catalogue`] of [`Card`]s, matched to a model by a
//! part of its id:
//!
//! * the catalogue built in ([`BUILT_IN`], `models.toml` beside this file)
//!   holds Leon's starting estimate of each family's talents, on a scale of
//!   1 to 5, and no benchmark: a number without its source is not given;
//! * `king-models.toml` in Leon's data folder adds to it and wins over it.
//!   King writes there what it finds out (`model_note`), each benchmark with
//!   where it comes from and when, and so can the person.
//!
//! Pure: text in, cards and text out.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// The catalogue built in.
pub const BUILT_IN: &str = include_str!("models.toml");

/// The name of the person's and King's own catalogue in Leon's data folder.
pub const FILE_NAME: &str = "king-models.toml";

/// The talents a card rates, in the order they are shown.
pub const TALENTS: [&str; 7] = [
    "planning",
    "architecture",
    "coding",
    "review",
    "research",
    "speed",
    "economy",
];

/// Everything known of models' talents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    /// The cards; a later one with the same pattern replaces an earlier one.
    #[serde(default, rename = "model")]
    pub cards: Vec<Card>,
}

/// What is known of one model, or of a family of them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Card {
    /// A part of the model's id, in lower case (`opus`, `gpt-5`,
    /// `deepseek`); the longest that matches wins.
    #[serde(rename = "match")]
    pub pattern: String,
    /// Its name.
    pub name: String,
    /// Each talent of [`TALENTS`] from 1 (poor) to 5 (the best there is).
    #[serde(default)]
    pub talents: BTreeMap<String, u8>,
    /// Benchmarks by name (`swe-bench-verified`, `terminal-bench`).
    #[serde(default)]
    pub benchmarks: BTreeMap<String, Benchmark>,
    /// What it is best used for, and what to avoid it for.
    #[serde(default)]
    pub notes: Option<String>,
}

/// One benchmark's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Benchmark {
    /// The score.
    pub score: f64,
    /// Its unit, when it is not a percentage.
    #[serde(default)]
    pub unit: Option<String>,
    /// Where it was published.
    pub source: String,
    /// When it was read (`2026-10`).
    pub as_of: String,
}

/// A model an agent of this computer offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Available {
    /// The agent's tag.
    pub agent: String,
    /// The model's id, as a turn is started with it.
    pub id: String,
    /// Its name.
    pub label: String,
    /// What the agent says it is good for.
    pub description: Option<String>,
    /// The efforts it can be asked for, the lightest first.
    pub efforts: Vec<String>,
}

/// A permission mode an agent of this computer offers: how much it asks
/// before it acts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableMode {
    /// The agent's tag.
    pub agent: String,
    /// The mode's id, as a turn is started with it.
    pub id: String,
    /// Its name.
    pub label: String,
    /// What it means, in a sentence.
    pub description: Option<String>,
    /// It lets the agent do anything without asking.
    pub dangerous: bool,
}

impl Catalogue {
    /// Reads a catalogue's text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let catalogue: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        if let Some(card) = catalogue
            .cards
            .iter()
            .find(|card| card.pattern.trim().is_empty())
        {
            return Err(format!("the card {:?} matches nothing", card.name));
        }
        if let Some((card, talent)) = catalogue.cards.iter().find_map(|card| {
            card.talents
                .iter()
                .find(|(_, score)| !(1..=5).contains(*score))
                .map(|(talent, _)| (card, talent))
        }) {
            return Err(format!(
                "{}: the talent {talent:?} is rated from 1 to 5",
                card.name
            ));
        }
        Ok(catalogue)
    }

    /// The catalogue built in, then `own` over it.
    pub fn with(own: Option<&str>) -> Result<Self, String> {
        let mut catalogue = Self::parse(BUILT_IN)?;
        if let Some(text) = own {
            for card in Self::parse(text)?.cards {
                catalogue.put(card);
            }
        }
        Ok(catalogue)
    }

    /// Adds a card, or replaces the one with its pattern.
    pub fn put(&mut self, card: Card) {
        let pattern = card.pattern.to_lowercase();
        match self
            .cards
            .iter_mut()
            .find(|known| known.pattern.to_lowercase() == pattern)
        {
            Some(known) => *known = card,
            None => self.cards.push(card),
        }
    }

    /// The card of a model: the one whose pattern is the longest part of
    /// its id (or name) that matches.
    pub fn card_for(&self, id: &str, label: &str) -> Option<&Card> {
        let id = id.to_lowercase();
        let label = label.to_lowercase();
        self.cards
            .iter()
            .filter(|card| {
                let pattern = card.pattern.to_lowercase();
                id.contains(&pattern) || label.contains(&pattern)
            })
            .max_by_key(|card| card.pattern.len())
    }
}

impl Card {
    /// Its talents as one line: `planning 5, coding 5, …`.
    pub fn talent_line(&self) -> String {
        TALENTS
            .iter()
            .filter_map(|talent| {
                self.talents
                    .get(*talent)
                    .map(|score| format!("{talent} {score}"))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Its benchmarks as one line, each with its source and date.
    pub fn benchmark_line(&self) -> String {
        self.benchmarks
            .iter()
            .map(|(name, result)| {
                let unit = result.unit.as_deref().unwrap_or("%");
                format!(
                    "{name} {}{unit} ({}, {})",
                    result.score, result.source, result.as_of
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// The models of this computer with what is known of each, as King's
/// instructions list them.
pub fn render(available: &[Available], catalogue: &Catalogue) -> String {
    if available.is_empty() {
        return "No agent could be asked for its models yet.\n".to_owned();
    }
    let mut text = String::new();
    let mut by_agent: BTreeMap<&str, Vec<&Available>> = BTreeMap::new();
    for model in available {
        by_agent
            .entry(model.agent.as_str())
            .or_default()
            .push(model);
    }
    for (agent, models) in by_agent {
        let _ = writeln!(text, "* **{agent}**");
        for model in models {
            let efforts = match model.efforts.as_slice() {
                [] => String::new(),
                efforts => format!(" · efforts: {}", efforts.join(", ")),
            };
            let _ = writeln!(text, "  * `{}` ({}){efforts}", model.id, model.label);
            match catalogue.card_for(&model.id, &model.label) {
                Some(card) => {
                    let talents = card.talent_line();
                    if !talents.is_empty() {
                        let _ = writeln!(text, "    talents: {talents}");
                    }
                    let benchmarks = card.benchmark_line();
                    if !benchmarks.is_empty() {
                        let _ = writeln!(text, "    benchmarks: {benchmarks}");
                    }
                    if let Some(notes) = &card.notes {
                        let _ = writeln!(text, "    {notes}");
                    }
                }
                None => {
                    let _ = writeln!(
                        text,
                        "    nothing known of it yet: find out, and keep it with `model_note`"
                    );
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(agent: &str, id: &str, label: &str) -> Available {
        Available {
            agent: agent.into(),
            id: id.into(),
            label: label.into(),
            description: None,
            efforts: Vec::new(),
        }
    }

    #[test]
    fn the_built_in_catalogue_reads_and_names_no_benchmark_without_a_source() {
        let catalogue = Catalogue::with(None).unwrap();
        assert!(catalogue.cards.len() >= 8);
        assert!(catalogue
            .cards
            .iter()
            .all(|card| card.benchmarks.is_empty()));
        assert!(catalogue.cards.iter().all(|card| card
            .talents
            .keys()
            .all(|talent| TALENTS.contains(&talent.as_str()))));
    }

    #[test]
    fn the_longest_matching_part_of_the_id_wins_and_ones_own_card_replaces() {
        let own = r#"
            [[model]]
            match = "opus"
            name = "Claude Opus (as we measured)"
            talents = { planning = 5 }
            [model.benchmarks.swe-bench-verified]
            score = 80.0
            source = "anthropic.com"
            as_of = "2026-10"
        "#;
        let catalogue = Catalogue::with(Some(own)).unwrap();
        let card = catalogue.card_for("claude-opus-5-5", "Opus").unwrap();
        assert_eq!(card.name, "Claude Opus (as we measured)");
        assert_eq!(
            card.benchmark_line(),
            "swe-bench-verified 80% (anthropic.com, 2026-10)"
        );
        assert!(catalogue.card_for("opencode/deepseek-v4", "").is_some());
        assert!(catalogue.card_for("mystery-model", "Mystery").is_none());
    }

    #[test]
    fn a_rating_out_of_its_scale_is_refused() {
        let bad = "[[model]]\nmatch = \"x\"\nname = \"X\"\ntalents = { coding = 9 }";
        assert!(Catalogue::parse(bad).is_err());
        assert!(Catalogue::parse("[[model]]\nmatch = \"\"\nname = \"X\"").is_err());
    }

    #[test]
    fn the_models_are_listed_by_agent_with_what_is_known() {
        let catalogue = Catalogue::with(None).unwrap();
        let text = render(
            &[
                model("claude", "opus", "Opus"),
                model("opencode", "acme/unknown-1", "Unknown"),
            ],
            &catalogue,
        );
        assert!(text.contains("* **claude**\n  * `opus` (Opus)"));
        assert!(text.contains("talents: planning"));
        assert!(text.contains("nothing known of it yet"));
        assert!(render(&[], &catalogue).contains("could be asked"));
    }
}
