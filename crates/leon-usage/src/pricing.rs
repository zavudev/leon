//! What a model's tokens would cost on its provider's API.
//!
//! The prices are data, not code: a JSON file bundled with this crate
//! (`data/prices.json`) holds one entry per model, in US dollars per million
//! tokens, with the page it was copied from and the day it was recorded. The
//! user can override or extend it with a `prices.json` next to the settings
//! file, in the same format; [`PriceTable::with_overrides`] merges the two.
//!
//! Everything here is pure. The result is an **estimate of the API's list
//! price** for the tokens counted, not what a subscription costs: a model with
//! no entry has no price ([`PriceTable::cost`] is `None`), and nothing is ever
//! guessed for it.
//!
//! A model is found by its name after [`normalize_model`]: lower case, with no
//! provider prefix, no `[1m]` context marker and no `-YYYYMMDD` snapshot date,
//! and with dots and hyphens alike, so `claude-opus-4-5-20251101`,
//! `anthropic/claude-opus-4.5` and `claude-opus-4-5` are one model. An entry
//! can list further names in `aliases`.
//!
//! Where an entry gives no price for the cache, the input price is used: a
//! cache read or write is then billed as ordinary input (an upper bound). The
//! one-hour cache write falls back to the five-minute one.

use leon_core::TokenCounts;
use serde::Deserialize;

/// The prices that ship with Leon.
const BUNDLED: &str = include_str!("../data/prices.json");

/// The name of the user's file, next to the settings file.
pub const FILE_NAME: &str = "prices.json";

/// One model's prices, in US dollars per million tokens.
#[derive(Clone, Debug, PartialEq)]
pub struct PriceEntry {
    /// The model's name, normalised.
    pub id: String,
    /// What the provider calls it, for display.
    pub name: Option<String>,
    /// Other names the model goes by in transcripts, normalised.
    pub aliases: Vec<String>,
    /// Input that was not served from the cache.
    pub input: f64,
    /// Output, reasoning included.
    pub output: f64,
    /// Input served from the cache; the input price when absent.
    pub cache_read: Option<f64>,
    /// Input written to the cache (five minutes); the input price when absent.
    pub cache_write: Option<f64>,
    /// Input written to the one-hour cache; the five-minute price when absent.
    pub cache_write_1h: Option<f64>,
    /// Where the prices were copied from.
    pub source: String,
    /// The day they were copied (`YYYY-MM-DD`).
    pub recorded: String,
    /// What the figure leaves out, in the file's own words.
    pub note: Option<String>,
}

impl PriceEntry {
    /// The estimated cost of `counts` at this entry's prices, in US dollars.
    pub fn cost(&self, counts: &TokenCounts) -> f64 {
        let cache_write = self.cache_write.unwrap_or(self.input);
        let per = |tokens: u64, price: f64| tokens as f64 * price;
        let micro = per(counts.input, self.input)
            + per(counts.output, self.output)
            + per(counts.cache_read, self.cache_read.unwrap_or(self.input))
            + per(counts.cache_write, cache_write)
            + per(
                counts.cache_write_1h,
                self.cache_write_1h.unwrap_or(cache_write),
            );
        micro / 1_000_000.0
    }

    fn matches(&self, key: &str) -> bool {
        self.id == key || self.aliases.iter().any(|alias| alias == key)
    }
}

/// The prices in force: the user's entries first, then Leon's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceTable {
    entries: Vec<PriceEntry>,
}

/// The file's shape.
#[derive(Deserialize)]
struct File {
    #[serde(default)]
    models: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Raw {
    id: String,
    name: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    cache_write_1h: Option<f64>,
    #[serde(default)]
    source: String,
    #[serde(default)]
    recorded: String,
    note: Option<String>,
}

impl Raw {
    /// The entry, or what is wrong with it.
    fn entry(self) -> Result<PriceEntry, String> {
        let prices = [
            Some(self.input),
            Some(self.output),
            self.cache_read,
            self.cache_write,
            self.cache_write_1h,
        ];
        if prices
            .into_iter()
            .flatten()
            .any(|price| !price.is_finite() || price < 0.0)
        {
            return Err(format!("{}: a price is negative or not a number", self.id));
        }
        let id = normalize_model(&self.id);
        if id.is_empty() {
            return Err("an entry has no id".to_owned());
        }
        Ok(PriceEntry {
            id,
            name: self.name,
            aliases: self
                .aliases
                .iter()
                .map(|alias| normalize_model(alias))
                .filter(|alias| !alias.is_empty())
                .collect(),
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            cache_write_1h: self.cache_write_1h,
            source: self.source,
            recorded: self.recorded,
            note: self.note,
        })
    }
}

/// The entries of a price file, and what was wrong with the ones left out.
fn parse(text: &str) -> Result<(Vec<PriceEntry>, Vec<String>), String> {
    let file: File = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    for (index, value) in file.models.into_iter().enumerate() {
        match serde_json::from_value::<Raw>(value).map_err(|error| error.to_string()) {
            Ok(raw) => match raw.entry() {
                Ok(entry) => entries.push(entry),
                Err(problem) => problems.push(problem),
            },
            Err(error) => problems.push(format!("entry {}: {error}", index + 1)),
        }
    }
    Ok((entries, problems))
}

impl PriceTable {
    /// No prices at all: every model reads "no price".
    pub fn empty() -> Self {
        Self::default()
    }

    /// The prices that ship with Leon.
    pub fn bundled() -> Self {
        // The bundled file is checked by a test, so a failure here cannot
        // reach a user; an empty table is the honest fallback anyway.
        parse(BUNDLED)
            .map(|(entries, _)| Self { entries })
            .unwrap_or_default()
    }

    /// This table with the entries of the user's file `text` on top: an entry
    /// for a model already priced replaces it, any other is added. The
    /// problems are the entries of the file that could not be used; a file
    /// that is not JSON at all is an `Err` and the table stays as it was.
    pub fn with_overrides(mut self, text: &str) -> Result<(Self, Vec<String>), String> {
        let (user, problems) = parse(text)?;
        self.entries
            .retain(|kept| !user.iter().any(|entry| entry.id == kept.id));
        let mut entries = user;
        entries.append(&mut self.entries);
        Ok((Self { entries }, problems))
    }

    /// Every entry, the user's first.
    pub fn entries(&self) -> &[PriceEntry] {
        &self.entries
    }

    /// The entry for a model as a transcript names it.
    pub fn lookup(&self, model: &str) -> Option<&PriceEntry> {
        let key = normalize_model(model);
        if key.is_empty() {
            return None;
        }
        self.entries.iter().find(|entry| entry.matches(&key))
    }

    /// The estimated cost of `counts` of `model`, in US dollars; `None` for a
    /// model with no price.
    pub fn cost(&self, model: &str, counts: &TokenCounts) -> Option<f64> {
        self.lookup(model).map(|entry| entry.cost(counts))
    }
}

/// A model's name reduced to what identifies it; see the module comment.
pub fn normalize_model(model: &str) -> String {
    let mut name = model.trim().to_lowercase();
    if let Some(at) = name.rfind('/') {
        name.drain(..=at);
    }
    if let Some(at) = name.find('[') {
        name.truncate(at);
    }
    let name = name.trim_end_matches('-');
    let name = match name.rsplit_once('-') {
        Some((rest, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => name,
    };
    name.replace(['.', ' ', '_'], "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(input: u64, output: u64, read: u64, write: u64, hour: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            cache_read: read,
            cache_write: write,
            cache_write_1h: hour,
        }
    }

    const FILE: &str = r#"{"models": [
        {"id": "model-a", "name": "Model A", "aliases": ["model-alpha"],
         "input": 5, "output": 25, "cache_read": 0.5, "cache_write": 6.25,
         "cache_write_1h": 10, "source": "https://example.test/a", "recorded": "2026-01-01"},
        {"id": "model-b", "input": 2, "output": 8,
         "source": "https://example.test/b", "recorded": "2026-01-01"}
    ]}"#;

    fn table() -> PriceTable {
        PriceTable::empty().with_overrides(FILE).unwrap().0
    }

    #[test]
    fn names_are_reduced_to_what_identifies_the_model() {
        for name in [
            "claude-opus-4-5-20251101",
            "Claude-Opus-4.5",
            "anthropic/claude-opus-4-5",
            "claude-opus-4-5[1m]",
            " claude-opus-4-5 ",
        ] {
            assert_eq!(normalize_model(name), "claude-opus-4-5", "{name}");
        }
        // Only an eight digit snapshot date is dropped.
        assert_eq!(normalize_model("gpt-5.3-codex"), "gpt-5-3-codex");
        assert_eq!(normalize_model("model-2026"), "model-2026");
        assert_eq!(normalize_model(""), "");
    }

    #[test]
    fn the_arithmetic_is_per_million_tokens_with_each_kind_at_its_price() {
        // A recorded reply: 2 fresh input, 54 output, 30516 cache read and
        // 14227 one-hour cache write.
        let cost = table()
            .cost("model-a", &counts(2, 54, 30_516, 0, 14_227))
            .unwrap();
        let expected = (2.0 * 5.0 + 54.0 * 25.0 + 30_516.0 * 0.5 + 14_227.0 * 10.0) / 1e6;
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");
        let both = table()
            .cost("model-a", &counts(0, 0, 0, 1_000_000, 1_000_000))
            .unwrap();
        assert!((both - 16.25).abs() < 1e-9);
        assert_eq!(table().cost("model-a", &TokenCounts::default()), Some(0.0));
    }

    #[test]
    fn a_missing_cache_price_is_billed_as_input_and_the_hour_write_as_the_short_one() {
        let table = table();
        let plain = table
            .cost("model-b", &counts(0, 0, 1_000_000, 1_000_000, 1_000_000))
            .unwrap();
        assert!((plain - 6.0).abs() < 1e-9);
    }

    #[test]
    fn a_model_with_no_entry_has_no_price_not_a_guess() {
        let table = table();
        assert_eq!(table.cost("some-other-model", &counts(1, 1, 1, 1, 1)), None);
        assert_eq!(table.cost("", &counts(1, 1, 1, 1, 1)), None);
        assert!(PriceTable::empty().lookup("model-a").is_none());
        // Bare family names are ambiguous and are not priced.
        assert!(PriceTable::bundled().lookup("opus").is_none());
        assert!(PriceTable::bundled().lookup("inherit").is_none());
    }

    #[test]
    fn a_model_is_found_by_its_id_its_aliases_and_a_snapshot_date() {
        let table = table();
        for name in [
            "model-a",
            "MODEL-A",
            "model-alpha",
            "model-a-20260101",
            "model-a[1m]",
        ] {
            assert_eq!(table.lookup(name).unwrap().id, "model-a", "{name}");
        }
    }

    #[test]
    fn the_users_file_replaces_what_it_names_and_adds_the_rest() {
        let user = r#"{"models": [
            {"id": "model-a", "input": 1, "output": 2},
            {"id": "model-c", "input": 3, "output": 4, "aliases": ["c"]}
        ]}"#;
        let (merged, problems) = table().with_overrides(user).unwrap();
        assert!(problems.is_empty());
        assert_eq!(merged.lookup("model-a").unwrap().input, 1.0);
        assert_eq!(
            merged.lookup("model-alpha"),
            None,
            "the old entry is gone with its aliases"
        );
        assert_eq!(merged.lookup("c").unwrap().id, "model-c");
        assert_eq!(merged.lookup("model-b").unwrap().input, 2.0);
        assert_eq!(merged.entries().len(), 3);
    }

    #[test]
    fn a_bad_entry_is_reported_and_left_out_and_a_bad_file_changes_nothing() {
        let user = r#"{"models": [
            {"id": "ok", "input": 1, "output": 2},
            {"id": "negative", "input": -1, "output": 2},
            {"id": "no-output", "input": 1},
            {"input": 1, "output": 1},
            {"id": "", "input": 1, "output": 1}
        ]}"#;
        let (merged, problems) = PriceTable::empty().with_overrides(user).unwrap();
        assert_eq!(merged.entries().len(), 1);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems[0].contains("negative"));
        assert!(PriceTable::empty().with_overrides("{not json").is_err());
        assert!(PriceTable::empty()
            .with_overrides("{}")
            .unwrap()
            .0
            .entries()
            .is_empty());
    }

    #[test]
    fn every_bundled_price_says_where_and_when_it_was_copied() {
        let bundled = PriceTable::bundled();
        assert!(!bundled.entries().is_empty());
        let (_, problems) = parse(BUNDLED).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        for entry in bundled.entries() {
            assert!(entry.source.starts_with("https://"), "{}", entry.id);
            assert_eq!(entry.recorded.len(), 10, "{}", entry.id);
            assert!(entry.output >= entry.input, "{}", entry.id);
            assert!(entry.cache_read.is_some(), "{}", entry.id);
        }
        // No two entries answer to one name.
        for entry in bundled.entries() {
            assert_eq!(bundled.lookup(&entry.id).unwrap().id, entry.id);
        }
    }

    #[test]
    fn the_names_agents_write_find_their_bundled_entries() {
        let bundled = PriceTable::bundled();
        for (name, id) in [
            ("claude-opus-5-5", "claude-opus-5-5"),
            ("claude-opus-5[1m]", "claude-opus-5"),
            ("claude-haiku-4-5-20251001", "claude-haiku-4-5"),
            ("claude-3-5-haiku-20241022", "claude-haiku-3-5"),
            ("gpt-5.6-sol", "gpt-5-6-sol"),
        ] {
            assert_eq!(
                bundled.lookup(name).map(|e| e.id.as_str()),
                Some(id),
                "{name}"
            );
        }
        // A model whose price depends on the length of each prompt is not listed.
        assert!(bundled.lookup("claude-haiku-5-5").is_none());
    }
}
