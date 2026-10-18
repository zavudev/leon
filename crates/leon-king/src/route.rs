//! Which agent and model take a job.
//!
//! King asks for a worker either by tier (`cheap`, `standard`, `premium`, or
//! any the person named in `king.toml`) or by agent and model. Either way
//! the answer depends on where each agent stands now: whether it is
//! installed, and how much of its usage window it has used (the share Leon
//! reads for a subscription, with what is forecast to be used before the
//! window resets). A tier takes the first of its candidates with room. An
//! agent King names is taken as named, unless it is not installed or out of
//! room; then the answer says why and which agent would do instead, so King
//! can decide. Nothing is started once the day's budget is spent.
//!
//! Pure: standings in, a choice out.

use serde::{Deserialize, Serialize};

use crate::config::{Candidate, Config, DEFAULT_TIER, PLANNING_TIER};

/// Where one agent stands now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    /// Its catalogue tag.
    pub agent: String,
    /// Whether it can be started.
    pub installed: bool,
    /// The largest share (0 to 1) of any of its usage windows, forecast to
    /// the window's reset; nothing when Leon cannot tell.
    pub used: Option<f64>,
}

/// What King asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    /// A tier, by name.
    pub tier: Option<String>,
    /// An agent, by tag; it wins over the tier.
    pub agent: Option<String>,
    /// The model, with an agent.
    pub model: Option<String>,
    /// How hard it thinks, with an agent.
    pub effort: Option<String>,
    /// It is a plan: only the planning tier's models take it.
    pub planning: bool,
}

/// The agent and model a job goes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// The agent's tag.
    pub agent: String,
    /// The model; the agent's own when absent.
    pub model: Option<String>,
    /// How hard it thinks; the agent's own when absent.
    pub effort: Option<String>,
    /// The tier it was chosen from, when it was.
    pub tier: Option<String>,
}

/// Picks the agent for a job, or says why there is none.
pub fn route(
    config: &Config,
    ask: &Ask,
    standings: &[Standing],
    spent_today_usd: f64,
) -> Result<Choice, String> {
    if let Some(budget) = config.limits.daily_budget_usd {
        if spent_today_usd >= budget {
            return Err(format!(
                "today's budget of ${budget:.2} is spent (${spent_today_usd:.2}); ask the \
                 person before starting more work"
            ));
        }
    }
    if ask.planning {
        return plan(config, ask, standings, spent_today_usd);
    }
    let threshold = config.limits.limit_threshold;
    let room = |agent: &str| -> Result<(), String> {
        let Some(standing) = standings.iter().find(|standing| standing.agent == agent) else {
            return Err(format!("{agent} is not an agent Leon can drive"));
        };
        if !standing.installed {
            return Err(format!("{agent} is not installed"));
        }
        match standing.used {
            Some(used) if used >= threshold => Err(format!(
                "{agent} has used {:.0}% of its usage window (the line is {:.0}%)",
                used * 100.0,
                threshold * 100.0
            )),
            _ => Ok(()),
        }
    };
    if let Some(agent) = ask.agent.as_deref().filter(|agent| !agent.is_empty()) {
        return match room(agent) {
            Ok(()) => Ok(Choice {
                agent: agent.to_owned(),
                model: ask.model.clone(),
                effort: ask.effort.clone(),
                tier: None,
            }),
            Err(why) => {
                let instead = config
                    .tiers
                    .values()
                    .flatten()
                    .find(|candidate| candidate.agent != agent && room(&candidate.agent).is_ok())
                    .map(|candidate| format!("; {} has room", describe(candidate)))
                    .unwrap_or_default();
                Err(format!("{why}{instead}"))
            }
        };
    }
    let tier = ask
        .tier
        .as_deref()
        .filter(|tier| !tier.is_empty())
        .unwrap_or(DEFAULT_TIER);
    let Some(candidates) = config.tiers.get(tier) else {
        let known: Vec<&str> = config.tiers.keys().map(String::as_str).collect();
        return Err(format!(
            "there is no tier {tier:?}; the tiers are {}",
            known.join(", ")
        ));
    };
    let mut reasons = Vec::new();
    for candidate in candidates {
        match room(&candidate.agent) {
            Ok(()) => {
                return Ok(Choice {
                    agent: candidate.agent.clone(),
                    model: candidate.model.clone(),
                    effort: candidate.effort.clone(),
                    tier: Some(tier.to_owned()),
                })
            }
            Err(why) => reasons.push(why),
        }
    }
    Err(format!(
        "no agent of the tier {tier:?} can take it: {}",
        reasons.join("; ")
    ))
}

/// A plan: on the planning tier (the premium one when the file names no
/// planning tier), and on a named agent only when the tier has it.
fn plan(
    config: &Config,
    ask: &Ask,
    standings: &[Standing],
    spent_today_usd: f64,
) -> Result<Choice, String> {
    let tier = if config.tiers.contains_key(PLANNING_TIER) {
        PLANNING_TIER
    } else {
        "premium"
    };
    let candidates = config.tiers.get(tier).cloned().unwrap_or_default();
    let mut config = config.clone();
    if let Some(agent) = ask.agent.as_deref().filter(|agent| !agent.is_empty()) {
        let named: Vec<Candidate> = candidates
            .iter()
            .filter(|candidate| {
                candidate.agent == agent
                    && ask.model.as_deref().is_none_or(|model| {
                        candidate.model.as_deref().is_none_or(|own| own == model)
                    })
            })
            .cloned()
            .collect();
        if named.is_empty() {
            let names: Vec<String> = candidates.iter().map(describe).collect();
            return Err(format!(
                "plans go to the best models only, the {tier} tier: {}",
                names.join(", ")
            ));
        }
        config.tiers.insert(tier.to_owned(), named);
    }
    let ask = Ask {
        tier: Some(tier.to_owned()),
        ..Ask::default()
    };
    route(&config, &ask, standings, spent_today_usd)
}

fn describe(candidate: &Candidate) -> String {
    match &candidate.model {
        Some(model) => format!("{} ({model})", candidate.agent),
        None => candidate.agent.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(agent: &str, installed: bool, used: Option<f64>) -> Standing {
        Standing {
            agent: agent.to_owned(),
            installed,
            used,
        }
    }

    fn everyone() -> Vec<Standing> {
        ["claude", "codex", "opencode", "grok", "cursor"]
            .iter()
            .map(|agent| standing(agent, true, Some(0.1)))
            .collect()
    }

    fn tier(name: &str) -> Ask {
        Ask {
            tier: Some(name.to_owned()),
            ..Ask::default()
        }
    }

    #[test]
    fn a_tier_takes_its_first_candidate_with_room() {
        let config = Config::default();
        let chosen = route(&config, &tier("cheap"), &everyone(), 0.0).unwrap();
        assert_eq!(chosen.agent, "opencode");
        assert_eq!(chosen.tier.as_deref(), Some("cheap"));

        let mut standings = everyone();
        standings[2] = standing("opencode", false, None);
        let chosen = route(&config, &tier("cheap"), &standings, 0.0).unwrap();
        assert_eq!(chosen.agent, "cursor");
    }

    #[test]
    fn an_agent_near_its_limit_is_passed_over_and_one_leon_cannot_read_is_not() {
        let config = Config::default();
        let mut standings = everyone();
        standings[1] = standing("codex", true, Some(0.9));
        standings[0] = standing("claude", true, None);
        let chosen = route(&config, &Ask::default(), &standings, 0.0).unwrap();
        // The default tier is standard: codex first, then claude.
        assert_eq!(chosen.agent, "claude");
        assert_eq!(chosen.tier.as_deref(), Some("standard"));
    }

    #[test]
    fn a_named_agent_is_taken_as_named_or_refused_with_one_that_would_do() {
        let config = Config::default();
        let ask = Ask {
            agent: Some("grok".into()),
            model: Some("grok-5".into()),
            ..Ask::default()
        };
        let chosen = route(&config, &ask, &everyone(), 0.0).unwrap();
        assert_eq!(
            (chosen.agent.as_str(), chosen.model.as_deref()),
            ("grok", Some("grok-5"))
        );
        assert_eq!(chosen.tier, None);

        let mut standings = everyone();
        standings[3] = standing("grok", true, Some(0.97));
        let refused = route(&config, &ask, &standings, 0.0).unwrap_err();
        assert!(refused.contains("97%"), "{refused}");
        assert!(refused.contains("opencode has room"), "{refused}");
    }

    #[test]
    fn a_plan_goes_to_the_best_models_whatever_was_asked() {
        let config = Config::default();
        let cheap_plan = Ask {
            tier: Some("cheap".into()),
            planning: true,
            ..Ask::default()
        };
        let chosen = route(&config, &cheap_plan, &everyone(), 0.0).unwrap();
        assert_eq!(chosen.agent, "claude");
        assert_eq!(chosen.model.as_deref(), Some("opus"));
        assert_eq!(chosen.tier.as_deref(), Some("planning"));

        // Out of room: the next best, never a cheaper one.
        let mut standings = everyone();
        standings[0] = standing("claude", true, Some(0.99));
        let chosen = route(&config, &cheap_plan, &standings, 0.0).unwrap();
        assert_eq!(chosen.agent, "codex");
        assert_eq!(chosen.effort.as_deref(), Some("high"));

        let named_cheap = Ask {
            agent: Some("opencode".into()),
            planning: true,
            ..Ask::default()
        };
        let refused = route(&config, &named_cheap, &everyone(), 0.0).unwrap_err();
        assert!(refused.contains("best models only"), "{refused}");
        let named_codex = Ask {
            agent: Some("codex".into()),
            planning: true,
            ..Ask::default()
        };
        assert_eq!(
            route(&config, &named_codex, &everyone(), 0.0)
                .unwrap()
                .agent,
            "codex"
        );
    }

    #[test]
    fn nothing_starts_once_the_budget_is_spent_or_without_an_agent() {
        let mut config = Config::default();
        config.limits.daily_budget_usd = Some(10.0);
        let refused = route(&config, &tier("cheap"), &everyone(), 10.5).unwrap_err();
        assert!(refused.contains("budget"), "{refused}");
        assert!(route(&config, &tier("cheap"), &everyone(), 9.0).is_ok());

        let unknown = route(&config, &tier("gold"), &everyone(), 0.0).unwrap_err();
        assert!(
            unknown.contains("cheap, planning, premium, standard"),
            "{unknown}"
        );
        let none: Vec<Standing> = everyone()
            .into_iter()
            .map(|s| standing(&s.agent, false, None))
            .collect();
        let refused = route(&config, &tier("premium"), &none, 0.0).unwrap_err();
        assert!(refused.contains("claude is not installed"), "{refused}");
    }
}
