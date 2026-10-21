//! Which agent and model take a job.
//!
//! King asks for a worker either by tier (any `king.toml` names) or by agent
//! and model; asking for neither is the tier the job's kind goes to
//! (`[jobs]`). A kind of job marked `strict` (plans, by default) only ever
//! runs on its tier's models: a cheaper tier or agent asked for it is
//! refused. Either way
//! the answer depends on where each agent stands now: whether it is
//! installed, and how much of its usage window it has used (the share Leon
//! reads for a subscription, with what is forecast to be used before the
//! window resets). A tier takes the first of its candidates with room. An
//! agent King names is taken as named, unless it is not installed or out of
//! room; then the answer says why and which agent would do instead, so King
//! can decide. Nothing is started once the day's budget is spent.
//!
//! A tier also reads the record ([`crate::record`]): a candidate whose model
//! did poorly at this kind of job (of at least `[limits] learn_after`
//! workers judged, fewer than `learn_below` did well) is passed over while a
//! later one has room, and the answer says so. When none has, the tier's
//! order wins: the first with room takes the job all the same. An agent
//! King names is taken as named; King reads the record in `models_list`.
//!
//! Pure: standings and the record in, a choice out.

use serde::{Deserialize, Serialize};

use crate::config::{Candidate, Config};
use crate::record::{self, Summary};

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
    /// The kind of job (`code`, `research`, `plan`): where it goes when
    /// no tier is named, and whether it may go anywhere else.
    pub job: String,
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
    learned: &[Summary],
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
    let rule = config.job(&ask.job);
    if rule.strict {
        return strict(config, &rule.tier, ask, standings, learned, spent_today_usd);
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
        .unwrap_or(&rule.tier);
    let Some(candidates) = config.tiers.get(tier) else {
        let known: Vec<&str> = config.tiers.keys().map(String::as_str).collect();
        return Err(format!(
            "there is no tier {tier:?}; the tiers are {}",
            known.join(", ")
        ));
    };
    let choice = |candidate: &Candidate| Choice {
        agent: candidate.agent.clone(),
        model: candidate.model.clone(),
        effort: candidate.effort.clone(),
        tier: Some(tier.to_owned()),
    };
    let poor = |candidate: &Candidate| poor(config, learned, &ask.job, candidate);
    let mut reasons = Vec::new();
    let mut passed_over = None;
    for candidate in candidates {
        match room(&candidate.agent) {
            Ok(()) => match poor(candidate) {
                Some(why) => {
                    reasons.push(why);
                    passed_over.get_or_insert(candidate);
                }
                None => return Ok(choice(candidate)),
            },
            Err(why) => reasons.push(why),
        }
    }
    // Every one with room did poorly: the person's order decides.
    if let Some(candidate) = passed_over {
        return Ok(choice(candidate));
    }
    Err(format!(
        "no agent of the tier {tier:?} can take it: {}",
        reasons.join("; ")
    ))
}

/// Why a candidate is passed over for `job` on its record, when it is.
fn poor(config: &Config, learned: &[Summary], job: &str, candidate: &Candidate) -> Option<String> {
    let summary = record::of(learned, &candidate.agent, candidate.model.as_deref(), job)?;
    let rate = summary.rate(config.limits.learn_after)?;
    (rate < config.limits.learn_below).then(|| {
        format!(
            "{} did poorly at {job} jobs: {}",
            describe(candidate),
            summary.line()
        )
    })
}

/// The candidates of the tier `choice` came from that are before it and
/// were passed over on their record: what King is told the job did not go
/// to, and why.
pub fn passed_over(
    config: &Config,
    learned: &[Summary],
    job: &str,
    choice: &Choice,
) -> Vec<String> {
    let Some(candidates) = choice.tier.as_ref().and_then(|tier| config.tiers.get(tier)) else {
        return Vec::new();
    };
    candidates
        .iter()
        .take_while(|candidate| candidate.agent != choice.agent || candidate.model != choice.model)
        .filter_map(|candidate| poor(config, learned, job, candidate))
        .collect()
}

/// A job that only runs on `tier`'s models: on a named agent only when the
/// tier has it.
fn strict(
    config: &Config,
    tier: &str,
    ask: &Ask,
    standings: &[Standing],
    learned: &[Summary],
    spent_today_usd: f64,
) -> Result<Choice, String> {
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
                "a {} job only runs on the {tier} tier: {}",
                ask.job,
                names.join(", ")
            ));
        }
        config.tiers.insert(tier.to_owned(), named);
    }
    // The tier itself, as any job of no strict kind would have it.
    config.jobs.clear();
    config.jobs.insert(
        "code".to_owned(),
        crate::config::JobRule {
            tier: tier.to_owned(),
            strict: false,
        },
    );
    let ask = Ask {
        tier: Some(tier.to_owned()),
        job: ask.job.clone(),
        ..Ask::default()
    };
    route(&config, &ask, standings, learned, spent_today_usd)
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
        let chosen = route(&config, &tier("cheap"), &everyone(), &[], 0.0).unwrap();
        assert_eq!(chosen.agent, "opencode");
        assert_eq!(chosen.tier.as_deref(), Some("cheap"));

        let mut standings = everyone();
        standings[2] = standing("opencode", false, None);
        let chosen = route(&config, &tier("cheap"), &standings, &[], 0.0).unwrap();
        assert_eq!(chosen.agent, "cursor");
    }

    #[test]
    fn an_agent_near_its_limit_is_passed_over_and_one_leon_cannot_read_is_not() {
        let config = Config::default();
        let mut standings = everyone();
        standings[1] = standing("codex", true, Some(0.9));
        standings[0] = standing("claude", true, None);
        let chosen = route(&config, &Ask::default(), &standings, &[], 0.0).unwrap();
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
        let chosen = route(&config, &ask, &everyone(), &[], 0.0).unwrap();
        assert_eq!(
            (chosen.agent.as_str(), chosen.model.as_deref()),
            ("grok", Some("grok-5"))
        );
        assert_eq!(chosen.tier, None);

        let mut standings = everyone();
        standings[3] = standing("grok", true, Some(0.97));
        let refused = route(&config, &ask, &standings, &[], 0.0).unwrap_err();
        assert!(refused.contains("97%"), "{refused}");
        assert!(refused.contains("opencode has room"), "{refused}");
    }

    #[test]
    fn a_plan_goes_to_the_best_models_whatever_was_asked() {
        let config = Config::default();
        let cheap_plan = Ask {
            tier: Some("cheap".into()),
            job: "plan".into(),
            ..Ask::default()
        };
        let chosen = route(&config, &cheap_plan, &everyone(), &[], 0.0).unwrap();
        assert_eq!(chosen.agent, "claude");
        assert_eq!(chosen.model.as_deref(), Some("opus"));
        assert_eq!(chosen.tier.as_deref(), Some("planning"));

        // Out of room: the next best, never a cheaper one.
        let mut standings = everyone();
        standings[0] = standing("claude", true, Some(0.99));
        let chosen = route(&config, &cheap_plan, &standings, &[], 0.0).unwrap();
        assert_eq!(chosen.agent, "codex");
        assert_eq!(chosen.effort.as_deref(), Some("high"));

        let named_cheap = Ask {
            agent: Some("opencode".into()),
            job: "plan".into(),
            ..Ask::default()
        };
        let refused = route(&config, &named_cheap, &everyone(), &[], 0.0).unwrap_err();
        assert!(
            refused.contains("a plan job only runs on the planning tier"),
            "{refused}"
        );
        let named_codex = Ask {
            agent: Some("codex".into()),
            job: "plan".into(),
            ..Ask::default()
        };
        assert_eq!(
            route(&config, &named_codex, &everyone(), &[], 0.0)
                .unwrap()
                .agent,
            "codex"
        );
    }

    #[test]
    fn where_a_kind_of_job_goes_is_the_configurations() {
        let config = Config::parse(
            "[tiers]\nplanning = [{ agent = \"grok\", model = \"grok-5\" }]\n\
             [jobs]\nresearch = { tier = \"premium\" }\ncode = { tier = \"cheap\", strict = true }",
        )
        .unwrap();
        let plan = Ask {
            job: "plan".into(),
            ..Ask::default()
        };
        let chosen = route(&config, &plan, &everyone(), &[], 0.0).unwrap();
        assert_eq!(
            (chosen.agent.as_str(), chosen.model.as_deref()),
            ("grok", Some("grok-5"))
        );
        let research = Ask {
            job: "research".into(),
            ..Ask::default()
        };
        assert_eq!(
            route(&config, &research, &everyone(), &[], 0.0)
                .unwrap()
                .agent,
            "claude"
        );
        // Strict code: the cheap tier, whatever is asked.
        let premium_code = Ask {
            tier: Some("premium".into()),
            ..Ask::default()
        };
        assert_eq!(
            route(&config, &premium_code, &everyone(), &[], 0.0)
                .unwrap()
                .tier
                .as_deref(),
            Some("cheap")
        );
    }

    #[test]
    fn nothing_starts_once_the_budget_is_spent_or_without_an_agent() {
        let mut config = Config::default();
        config.limits.daily_budget_usd = Some(10.0);
        let refused = route(&config, &tier("cheap"), &everyone(), &[], 10.5).unwrap_err();
        assert!(refused.contains("budget"), "{refused}");
        assert!(route(&config, &tier("cheap"), &everyone(), &[], 9.0).is_ok());

        let unknown = route(&config, &tier("gold"), &everyone(), &[], 0.0).unwrap_err();
        assert!(
            unknown.contains("cheap, planning, premium, standard"),
            "{unknown}"
        );
        let none: Vec<Standing> = everyone()
            .into_iter()
            .map(|s| standing(&s.agent, false, None))
            .collect();
        let refused = route(&config, &tier("premium"), &none, &[], 0.0).unwrap_err();
        assert!(refused.contains("claude is not installed"), "{refused}");
    }

    fn record_of(model: &str, did_well: usize, did_poorly: usize) -> Vec<Summary> {
        let outcomes: Vec<crate::record::Outcome> = (0..did_well + did_poorly)
            .map(|n| {
                let mut outcome =
                    crate::record::Outcome::new(n as u32, "opencode", Some(model), "code", None, 0);
                outcome.checked(n < did_well);
                outcome
            })
            .collect();
        crate::record::summarize(&outcomes)
    }

    #[test]
    fn a_tier_passes_over_a_model_that_did_poorly_while_another_has_room() {
        let mut config = Config::default();
        config.tiers.insert(
            "cheap".to_owned(),
            vec![
                Candidate {
                    agent: "opencode".to_owned(),
                    model: Some("flash".to_owned()),
                    effort: None,
                },
                Candidate {
                    agent: "codex".to_owned(),
                    model: None,
                    effort: None,
                },
            ],
        );
        let code = Ask {
            tier: Some("cheap".to_owned()),
            job: "code".to_owned(),
            ..Ask::default()
        };
        let poor = record_of("flash", 1, 5);
        let chosen = route(&config, &code, &everyone(), &poor, 0.0).unwrap();
        assert_eq!(chosen.agent, "codex", "flash did well 1 time of 6");
        let why = passed_over(&config, &poor, "code", &chosen);
        assert_eq!(why.len(), 1);
        assert!(
            why[0].starts_with("opencode (flash) did poorly at code jobs: 1 of 6"),
            "{why:?}"
        );
        // Too few judged yet, or a good record: the person's order stands.
        let few = record_of("flash", 0, 3);
        assert_eq!(
            route(&config, &code, &everyone(), &few, 0.0).unwrap().agent,
            "opencode"
        );
        let good = record_of("flash", 5, 1);
        assert_eq!(
            route(&config, &code, &everyone(), &good, 0.0)
                .unwrap()
                .agent,
            "opencode"
        );
        // Nobody else has room: it takes the job all the same.
        let only = vec![
            Standing {
                agent: "opencode".to_owned(),
                installed: true,
                used: None,
            },
            Standing {
                agent: "codex".to_owned(),
                installed: false,
                used: None,
            },
        ];
        assert_eq!(
            route(&config, &code, &only, &poor, 0.0).unwrap().agent,
            "opencode"
        );
        // Learning off.
        config.limits.learn_after = 0;
        assert_eq!(
            route(&config, &code, &everyone(), &poor, 0.0)
                .unwrap()
                .agent,
            "opencode"
        );
        // Another kind of job has a record of its own.
        config.limits.learn_after = 5;
        let research = Ask {
            job: "research".to_owned(),
            ..code
        };
        assert_eq!(
            route(&config, &research, &everyone(), &poor, 0.0)
                .unwrap()
                .agent,
            "opencode"
        );
    }
}
