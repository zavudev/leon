//! `king.toml`: the agent King runs on, its workers' tiers and its limits.
//!
//! The file lives in Leon's data folder, beside the store, so that the
//! service reads it with the window closed. Every key is optional: a missing
//! file is [`Config::default`], King on Claude Code with three tiers that
//! name the agents Leon can drive. A file that cannot be read is an error
//! with the line it stopped at, never a silent default.
//!
//! ```toml
//! [brain]
//! agent = "claude"
//! model = "opus"
//! effort = "high"
//!
//! [limits]
//! max_workers = 6
//! daily_budget_usd = 20.0
//! limit_threshold = 0.85
//!
//! [tiers]
//! cheap = [{ agent = "opencode" }, { agent = "cursor" }]
//! standard = [{ agent = "codex", effort = "medium" }]
//! premium = [{ agent = "claude", model = "opus", effort = "high" }]
//! planning = [{ agent = "claude", model = "opus", effort = "high" }]
//!
//! [modes]
//! claude = "acceptEdits"
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The name of the file in Leon's data folder.
pub const FILE_NAME: &str = "king.toml";

/// The tier a job is given when nobody names one.
pub const DEFAULT_TIER: &str = "standard";

/// The tier every plan goes to: the best models there are.
pub const PLANNING_TIER: &str = "planning";

/// Everything `king.toml` says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The agent King itself runs on.
    pub brain: Brain,
    /// What King may spend and start.
    pub limits: Limits,
    /// The tiers of workers by name, each an ordered list of candidates:
    /// the first one that is installed and has room in its limits is taken.
    pub tiers: BTreeMap<String, Vec<Candidate>>,
    /// The permission mode a worker of each agent starts in, by agent tag.
    /// An agent named nowhere starts in its own default mode, and asks.
    pub modes: BTreeMap<String, String>,
}

/// The agent King runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Brain {
    /// The agent's catalogue tag (`claude`, `codex`, `opencode`, `grok`,
    /// `cursor`).
    pub agent: String,
    /// The model, by the id the agent lists; the agent's own when absent.
    pub model: Option<String>,
    /// How hard it thinks; the agent's own when absent.
    pub effort: Option<String>,
    /// Its permission mode; the agent's own when absent.
    pub mode: Option<String>,
}

/// What King may spend and start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// How many workers may work at the same time.
    pub max_workers: usize,
    /// What the workers may cost in a day, in US dollars, as the agents
    /// report it; no ceiling when absent.
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

impl Default for Brain {
    fn default() -> Self {
        Self {
            agent: "claude".to_owned(),
            model: None,
            effort: None,
            mode: None,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_workers: 6,
            daily_budget_usd: None,
            limit_threshold: 0.85,
            wake_delay_secs: 5,
        }
    }
}

impl Candidate {
    /// A candidate of `agent` with its own model and effort.
    pub fn agent(agent: &str) -> Self {
        Self {
            agent: agent.to_owned(),
            model: None,
            effort: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let tiers = [
            (
                "cheap",
                vec![Candidate::agent("opencode"), Candidate::agent("cursor")],
            ),
            (
                DEFAULT_TIER,
                vec![Candidate::agent("codex"), Candidate::agent("claude")],
            ),
            ("premium", vec![Candidate::agent("claude")]),
            (
                PLANNING_TIER,
                vec![
                    Candidate {
                        agent: "claude".to_owned(),
                        model: Some("opus".to_owned()),
                        effort: Some("high".to_owned()),
                    },
                    Candidate {
                        agent: "codex".to_owned(),
                        model: None,
                        effort: Some("high".to_owned()),
                    },
                ],
            ),
        ]
        .into_iter()
        .map(|(name, candidates)| (name.to_owned(), candidates))
        .collect();
        Self {
            brain: Brain::default(),
            limits: Limits::default(),
            tiers,
            modes: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Reads the text of `king.toml`. The error says what is wrong, and
    /// where.
    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        config.check()?;
        Ok(config)
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
        if let Some((name, _)) = self.tiers.iter().find(|(_, list)| list.is_empty()) {
            return Err(format!("the tier {name:?} names no agent"));
        }
        Ok(())
    }

    /// The permission mode a worker of `agent` starts in.
    pub fn mode_of(&self, agent: &str) -> Option<&str> {
        self.modes.get(agent).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_file_is_king_on_claude_with_three_tiers() {
        let config = Config::parse("").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.brain.agent, "claude");
        let tiers: Vec<&str> = config.tiers.keys().map(String::as_str).collect();
        assert_eq!(tiers, ["cheap", "planning", "premium", "standard"]);
        assert_eq!(config.limits.max_workers, 6);
    }

    #[test]
    fn the_documented_example_reads_as_it_says() {
        let text = r#"
            [brain]
            agent = "codex"
            model = "gpt-5.6"
            effort = "high"

            [limits]
            max_workers = 3
            daily_budget_usd = 20.0

            [tiers]
            cheap = [{ agent = "opencode", model = "deepseek/deepseek-v4" }]

            [modes]
            claude = "acceptEdits"
        "#;
        let config = Config::parse(text).unwrap();
        assert_eq!(config.brain.agent, "codex");
        assert_eq!(config.brain.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(config.limits.max_workers, 3);
        assert_eq!(config.limits.daily_budget_usd, Some(20.0));
        // A limit left out keeps its default.
        assert_eq!(config.limits.limit_threshold, 0.85);
        // Tiers named replace the default ones: only what the file says.
        assert_eq!(config.tiers.len(), 1);
        assert_eq!(
            config.tiers["cheap"][0].model.as_deref(),
            Some("deepseek/deepseek-v4")
        );
        assert_eq!(config.mode_of("claude"), Some("acceptEdits"));
        assert_eq!(config.mode_of("codex"), None);
    }

    #[test]
    fn a_mistake_is_said_and_never_read_as_the_default() {
        assert!(Config::parse("[brain]\nagnet = \"claude\"").is_err());
        assert!(Config::parse("[limits]\nlimit_threshold = 2.0").is_err());
        assert!(Config::parse("[limits]\nmax_workers = 0").is_err());
        assert!(Config::parse("[tiers]\ncheap = []").is_err());
        assert!(Config::parse("[brain]\nagent = \" \"").is_err());
    }
}
