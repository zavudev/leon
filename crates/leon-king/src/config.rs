//! `king.toml`: the agent King runs on, its workers' tiers, which tier each
//! kind of job goes to, its limits and its guards.
//!
//! Nothing of it is decided in code. Leon's defaults are a file of their own
//! ([`DEFAULTS`], `defaults.toml` beside this one, with a comment on every
//! key), and the person's `king.toml` in Leon's data folder is read over it
//! key by key ([`Config::parse`]): a table the person names changes only the
//! keys written in it, a tier named replaces that tier alone, and a tier set
//! to `[]` is removed. A file that cannot be read is an error with the line
//! it stopped at, never a silent default.
//!
//! ```toml
//! [brain]
//! model = "opus"
//!
//! [tiers]
//! planning = [{ agent = "claude", model = "fable", effort = "high" }]
//!
//! [jobs]
//! research = { tier = "standard" }
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The name of the person's file in Leon's data folder.
pub const FILE_NAME: &str = "king.toml";

/// Leon's defaults, as a file.
pub const DEFAULTS: &str = include_str!("defaults.toml");

/// Everything the configuration says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The agent King itself runs on.
    pub brain: Brain,
    /// What King may spend and start.
    pub limits: Limits,
    /// The tiers of workers by name, each an ordered list of candidates:
    /// the first one that is installed and has room in its limits is taken.
    pub tiers: BTreeMap<String, Vec<Candidate>>,
    /// Which tier each kind of job goes to, by the job's name (`code`,
    /// `research`, `plan`).
    pub jobs: BTreeMap<String, JobRule>,
    /// What King's tools never do.
    pub guards: Guards,
    /// The permission mode a worker of each agent starts in, by agent tag.
    /// An agent named nowhere starts in its own default mode, and asks.
    #[serde(default)]
    pub modes: BTreeMap<String, String>,
}

/// The agent King runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Brain {
    /// The agent's catalogue tag (`claude`, `codex`, `opencode`, `grok`,
    /// `cursor`).
    pub agent: String,
    /// The model, by the id the agent lists; the agent's own when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// How hard it thinks; the agent's own when absent.
    #[serde(default)]
    pub effort: Option<String>,
    /// Its permission mode; the agent's own when absent.
    #[serde(default)]
    pub mode: Option<String>,
}

/// What King may spend and start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// How many workers may work at the same time.
    pub max_workers: usize,
    /// What the workers and King may cost in a day, in US dollars, as the
    /// agents report it; no ceiling when absent.
    #[serde(default)]
    pub daily_budget_usd: Option<f64>,
    /// The share of a usage window (0 to 1) past which an agent on a
    /// subscription is not given new work.
    pub limit_threshold: f64,
    /// How long news of the workers is gathered before King is woken with
    /// it, in seconds.
    pub wake_delay_secs: u64,
}

/// One agent a tier may give work to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// The agent's catalogue tag.
    pub agent: String,
    /// The model; the agent's own when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// How hard it thinks; the agent's own when absent.
    #[serde(default)]
    pub effort: Option<String>,
}

/// Where one kind of job goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobRule {
    /// The tier it is given when King names none.
    pub tier: String,
    /// It only ever runs on this tier's models, whatever King asks for.
    #[serde(default)]
    pub strict: bool,
}

/// What King's tools never do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guards {
    /// Branches nothing is pushed to, besides each project's own default
    /// branch, which always is protected.
    pub protected_branches: Vec<String>,
}

impl Default for Config {
    /// Leon's defaults alone.
    fn default() -> Self {
        Self::parse("").expect("Leon's defaults are a valid configuration (tested)")
    }
}

impl Config {
    /// Reads the person's `king.toml` over Leon's defaults. The error says
    /// what is wrong, and where.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut merged: toml::Value =
            toml::from_str(DEFAULTS).map_err(|error| format!("Leon's defaults: {error}"))?;
        let own: toml::Value = toml::from_str(text).map_err(|error| error.to_string())?;
        merge(&mut merged, own);
        // A tier set to [] is removed.
        if let Some(tiers) = merged.get_mut("tiers").and_then(toml::Value::as_table_mut) {
            tiers.retain(|_, tier| tier.as_array().is_none_or(|list| !list.is_empty()));
        }
        let config: Self = merged.try_into().map_err(|error| error.to_string())?;
        config.check()?;
        Ok(config)
    }

    /// The configuration as a file, everything written out.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// What a file that parses may still get wrong.
    fn check(&self) -> Result<(), String> {
        if self.brain.agent.trim().is_empty() {
            return Err("[brain] agent must name an agent".to_owned());
        }
        if !(0.0..=1.0).contains(&self.limits.limit_threshold) {
            return Err("[limits] limit_threshold is a share, between 0 and 1".to_owned());
        }
        if self.limits.max_workers == 0 {
            return Err("[limits] max_workers must be at least 1".to_owned());
        }
        if self.tiers.is_empty() {
            return Err("[tiers] names no tier".to_owned());
        }
        for (job, rule) in &self.jobs {
            if !self.tiers.contains_key(&rule.tier) {
                return Err(format!(
                    "[jobs] {job} goes to the tier {:?}, which [tiers] does not name",
                    rule.tier
                ));
            }
        }
        if !self.jobs.contains_key("code") {
            return Err("[jobs] must say where code goes".to_owned());
        }
        Ok(())
    }

    /// The permission mode a worker of `agent` starts in.
    pub fn mode_of(&self, agent: &str) -> Option<&str> {
        self.modes.get(agent).map(String::as_str)
    }

    /// Where a kind of job goes; a job the file does not name goes where
    /// code does.
    pub fn job(&self, name: &str) -> &JobRule {
        self.jobs
            .get(name)
            .or_else(|| self.jobs.get("code"))
            .expect("[jobs] code is checked when the file is read")
    }
}

/// Writes `over` into `base`: tables key by key, anything else replaced.
fn merge(base: &mut toml::Value, over: toml::Value) {
    match (base, over) {
        (toml::Value::Table(base), toml::Value::Table(over)) => {
            for (key, value) in over {
                match base.get_mut(&key) {
                    Some(existing) if existing.is_table() && value.is_table() => {
                        merge(existing, value)
                    }
                    _ => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, over) => *base = over,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_file_is_leons_defaults_and_they_are_valid() {
        let config = Config::parse("").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.brain.agent, "claude");
        let tiers: Vec<&str> = config.tiers.keys().map(String::as_str).collect();
        assert_eq!(tiers, ["cheap", "planning", "premium", "standard"]);
        assert_eq!(config.job("plan").tier, "planning");
        assert!(config.job("plan").strict);
        assert_eq!(config.job("research").tier, "cheap");
        assert_eq!(config.job("anything else").tier, "standard");
        assert_eq!(config.guards.protected_branches, ["main", "master"]);
        // Written out and read back, it is the same.
        assert_eq!(Config::parse(&config.to_toml()).unwrap(), config);
    }

    #[test]
    fn ones_own_file_changes_only_what_it_names() {
        let text = r#"
            [brain]
            model = "opus"

            [limits]
            daily_budget_usd = 20.0

            [tiers]
            planning = [{ agent = "claude", model = "fable", effort = "high" }]
            premium = []
            local = [{ agent = "opencode", model = "ollama/gemma4:26b" }]

            [jobs]
            research = { tier = "local" }

            [modes]
            claude = "acceptEdits"
        "#;
        let config = Config::parse(text).unwrap();
        // A key left out of a table keeps its default.
        assert_eq!(config.brain.agent, "claude");
        assert_eq!(config.brain.model.as_deref(), Some("opus"));
        assert_eq!(config.limits.max_workers, 6);
        assert_eq!(config.limits.daily_budget_usd, Some(20.0));
        // A tier named replaces that tier alone; [] removes one.
        assert_eq!(config.tiers["planning"].len(), 1);
        assert_eq!(config.tiers["planning"][0].model.as_deref(), Some("fable"));
        assert_eq!(config.tiers["cheap"][0].agent, "opencode");
        assert!(!config.tiers.contains_key("premium"));
        assert_eq!(config.job("research").tier, "local");
        // A rule left alone keeps all of it.
        assert!(config.job("plan").strict);
        assert_eq!(config.mode_of("claude"), Some("acceptEdits"));
        assert_eq!(config.mode_of("codex"), None);
    }

    #[test]
    fn a_mistake_is_said_and_never_read_as_the_default() {
        assert!(Config::parse("[brain]\nagnet = \"claude\"").is_err());
        assert!(Config::parse("[limits]\nlimit_threshold = 2.0").is_err());
        assert!(Config::parse("[limits]\nmax_workers = 0").is_err());
        assert!(Config::parse("[brain]\nagent = \" \"").is_err());
        let orphan = Config::parse("[tiers]\nplanning = []").unwrap_err();
        assert!(
            orphan.contains("plan goes to the tier \"planning\""),
            "{orphan}"
        );
        assert!(Config::parse("[jobs]\nplan = { tier = \"gold\" }").is_err());
        assert!(Config::parse("brain = 3").is_err());
    }
}
