//! How each model did: the outcome of every worker, and what they say of a
//! model for a kind of job.
//!
//! The catalogue's talents ([`crate::models`]) say what a model is meant to
//! be good at; the record says how it did here, on these projects. Each
//! worker leaves an [`Outcome`] as things happen to it: each run of the
//! project's check and whether it passed, the CI of its pull request, a turn
//! that failed, what it cost and how long it took. [`summarize`] folds them
//! per agent, model and kind of job into a [`Summary`], which King reads in
//! `models_list`, and which the router ([`crate::route`]) uses to pass over a
//! model that keeps failing a kind of job.
//!
//! A worker is **judged** once something says how its work went: a check
//! ran, its CI ended, or it failed. It **did well** when it did not fail,
//! its last check passed (when it had one), and its CI did not fail. A worker
//! nothing judged (a question answered, research without a check) counts
//! among the workers and nowhere else.
//!
//! Pure: outcomes in, summaries out.

use serde::{Deserialize, Serialize};

/// How many outcomes are kept: the newest.
pub const KEPT: usize = 500;

/// What happened to one worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// The worker.
    pub worker: u32,
    /// Its agent's tag.
    pub agent: String,
    /// Its model; the agent's own when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// Its kind of job (`code`, `research`, `plan`).
    pub job: String,
    /// The project, when it was one's.
    #[serde(default)]
    pub project: Option<String>,
    /// When it started, in milliseconds since the epoch.
    pub started_ms: u64,
    /// How many times the project's check ran on its work.
    #[serde(default)]
    pub checks: u32,
    /// Whether its first check passed, once one ran.
    #[serde(default)]
    pub first_check: Option<bool>,
    /// Whether its last check passed, once one ran.
    #[serde(default)]
    pub last_check: Option<bool>,
    /// Whether the CI of its pull request passed, once it ended.
    #[serde(default)]
    pub ci: Option<bool>,
    /// Whether a turn of it failed.
    #[serde(default)]
    pub failed: bool,
    /// What it cost so far, in US dollars.
    #[serde(default)]
    pub cost_usd: f64,
    /// Whether its agent says what it costs: one that only runs in a
    /// terminal does not, and its cost is not known.
    #[serde(default = "priced")]
    pub priced: bool,
    /// How long it worked so far: from its start to the last thing that
    /// happened to it, in milliseconds.
    #[serde(default)]
    pub took_ms: u64,
}

impl Outcome {
    /// A worker's outcome before anything happened to it.
    pub fn new(
        worker: u32,
        agent: &str,
        model: Option<&str>,
        job: &str,
        project: Option<&str>,
        started_ms: u64,
    ) -> Self {
        Self {
            worker,
            agent: agent.to_owned(),
            model: model.map(str::to_owned),
            job: job.to_owned(),
            project: project.map(str::to_owned),
            started_ms,
            checks: 0,
            first_check: None,
            last_check: None,
            ci: None,
            failed: false,
            cost_usd: 0.0,
            priced: true,
            took_ms: 0,
        }
    }

    /// A run of the check ended.
    pub fn checked(&mut self, passed: bool) {
        self.checks += 1;
        self.first_check.get_or_insert(passed);
        self.last_check = Some(passed);
    }

    /// Whether anything says how its work went.
    pub fn judged(&self) -> bool {
        self.checks > 0 || self.ci.is_some() || self.failed
    }

    /// Whether its work went well, once judged.
    pub fn did_well(&self) -> bool {
        self.judged() && !self.failed && self.last_check != Some(false) && self.ci != Some(false)
    }
}

fn priced() -> bool {
    true
}

/// What the outcomes say of one model for one kind of job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// The agent's tag.
    pub agent: String,
    /// The model; the agent's own when absent.
    pub model: Option<String>,
    /// The kind of job.
    pub job: String,
    /// How many workers it was.
    pub workers: u32,
    /// How many of them something judged.
    pub judged: u32,
    /// How many of those did well.
    pub did_well: u32,
    /// How many had a check, and how many of those passed the first one.
    pub checked: u32,
    /// How many passed their first check.
    pub first_try: u32,
    /// How many had a CI that ended, and how many of those passed.
    pub ci_ended: u32,
    /// How many passed their CI.
    pub ci_passed: u32,
    /// How many had a turn fail.
    pub failed: u32,
    /// What a worker that did well cost, on average, in US dollars, when
    /// the agent says.
    pub cost_usd: Option<f64>,
    /// How long a worker that did well took, on average, in minutes.
    pub minutes: Option<f64>,
}

impl Summary {
    /// The share of the judged workers that did well, once enough were
    /// judged to say.
    pub fn rate(&self, enough: u32) -> Option<f64> {
        (enough > 0 && self.judged >= enough)
            .then(|| f64::from(self.did_well) / f64::from(self.judged))
    }

    /// The record in a few words: `4 of 6 did well, 3 of 5 checks passed
    /// first time, CI 2 of 2, $0.41 and 12 min each`.
    pub fn line(&self) -> String {
        let mut parts = vec![format!(
            "{} of {} judged did well ({} workers)",
            self.did_well, self.judged, self.workers
        )];
        if self.checked > 0 {
            parts.push(format!(
                "{} of {} passed their first check",
                self.first_try, self.checked
            ));
        }
        if self.ci_ended > 0 {
            parts.push(format!("CI passed {} of {}", self.ci_passed, self.ci_ended));
        }
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        match (self.cost_usd, self.minutes) {
            (Some(cost), Some(minutes)) => {
                parts.push(format!("${cost:.2} and {minutes:.0} min each"));
            }
            (None, Some(minutes)) => parts.push(format!("cost not known, {minutes:.0} min each")),
            _ => {}
        }
        parts.join(", ")
    }
}

/// The outcomes, per agent, model and kind of job, in the order each was
/// first seen.
pub fn summarize(outcomes: &[Outcome]) -> Vec<Summary> {
    let mut out: Vec<Summary> = Vec::new();
    // What the workers that did well cost (of those whose agent says, and
    // how many) and took.
    let mut spent: Vec<(f64, u32, u64)> = Vec::new();
    for outcome in outcomes {
        let at = match out.iter().position(|summary| {
            summary.agent == outcome.agent
                && summary.model == outcome.model
                && summary.job == outcome.job
        }) {
            Some(at) => at,
            None => {
                out.push(Summary {
                    agent: outcome.agent.clone(),
                    model: outcome.model.clone(),
                    job: outcome.job.clone(),
                    workers: 0,
                    judged: 0,
                    did_well: 0,
                    checked: 0,
                    first_try: 0,
                    ci_ended: 0,
                    ci_passed: 0,
                    failed: 0,
                    cost_usd: None,
                    minutes: None,
                });
                spent.push((0.0, 0, 0));
                out.len() - 1
            }
        };
        let summary = &mut out[at];
        summary.workers += 1;
        summary.judged += u32::from(outcome.judged());
        summary.checked += u32::from(outcome.checks > 0);
        summary.first_try += u32::from(outcome.first_check == Some(true));
        summary.ci_ended += u32::from(outcome.ci.is_some());
        summary.ci_passed += u32::from(outcome.ci == Some(true));
        summary.failed += u32::from(outcome.failed);
        if outcome.did_well() {
            summary.did_well += 1;
            if outcome.priced {
                spent[at].0 += outcome.cost_usd;
                spent[at].1 += 1;
            }
            spent[at].2 += outcome.took_ms;
        }
    }
    for (summary, (cost, priced, took)) in out.iter_mut().zip(spent) {
        if summary.did_well > 0 {
            let well = f64::from(summary.did_well);
            summary.cost_usd = (priced > 0).then(|| cost / f64::from(priced));
            summary.minutes = Some(took as f64 / 60_000.0 / well);
        }
    }
    out
}

/// What the record says of `agent`'s `model` for `job`.
pub fn of<'a>(
    summaries: &'a [Summary],
    agent: &str,
    model: Option<&str>,
    job: &str,
) -> Option<&'a Summary> {
    summaries.iter().find(|summary| {
        summary.agent == agent && summary.model.as_deref() == model && summary.job == job
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(worker: u32, model: &str) -> Outcome {
        Outcome::new(worker, "opencode", Some(model), "code", Some("api"), 0)
    }

    #[test]
    fn a_worker_is_judged_by_its_check_its_ci_and_its_failures() {
        let mut quiet = outcome(1, "flash");
        assert!(!quiet.judged() && !quiet.did_well());
        quiet.checked(false);
        assert!(quiet.judged() && !quiet.did_well(), "its check failed");
        quiet.checked(true);
        assert!(quiet.did_well(), "mended: its last check passed");
        assert_eq!(
            (quiet.checks, quiet.first_check, quiet.last_check),
            (2, Some(false), Some(true))
        );
        quiet.ci = Some(false);
        assert!(!quiet.did_well(), "its CI failed");
        let mut failed = outcome(2, "flash");
        failed.failed = true;
        assert!(failed.judged() && !failed.did_well());
    }

    #[test]
    fn outcomes_are_summed_per_model_and_job() {
        let mut first = outcome(1, "flash");
        first.checked(true);
        first.ci = Some(true);
        first.cost_usd = 0.2;
        first.took_ms = 10 * 60_000;
        let mut second = outcome(2, "flash");
        second.checked(false);
        second.checked(true);
        second.cost_usd = 0.4;
        second.took_ms = 20 * 60_000;
        let mut third = outcome(3, "flash");
        third.failed = true;
        let unjudged = outcome(4, "flash");
        let mut other = outcome(5, "glm");
        other.checked(true);
        let research = Outcome {
            job: "research".to_owned(),
            ..outcome(6, "flash")
        };
        let summaries = summarize(&[first, second, third, unjudged, other, research]);
        assert_eq!(summaries.len(), 3);
        let flash = of(&summaries, "opencode", Some("flash"), "code").unwrap();
        assert_eq!(
            (flash.workers, flash.judged, flash.did_well, flash.failed),
            (4, 3, 2, 1)
        );
        assert_eq!((flash.checked, flash.first_try), (2, 1));
        assert_eq!((flash.ci_ended, flash.ci_passed), (1, 1));
        assert!((flash.cost_usd.unwrap() - 0.3).abs() < 1e-9);
        assert!((flash.minutes.unwrap() - 15.0).abs() < 1e-9);
        assert_eq!(flash.rate(3), Some(2.0 / 3.0));
        assert_eq!(flash.rate(4), None, "not enough judged yet");
        assert_eq!(flash.rate(0), None, "learning is off");
        let line = flash.line();
        assert!(
            line.starts_with("2 of 3 judged did well (4 workers)"),
            "{line}"
        );
        assert!(line.contains("1 of 2 passed their first check"), "{line}");
        assert!(of(&summaries, "opencode", Some("glm"), "code").is_some());
        assert!(of(&summaries, "opencode", None, "code").is_none());
    }

    #[test]
    fn an_agent_that_says_no_cost_is_not_counted_as_free() {
        let mut terminal = Outcome::new(1, "kimi", None, "code", None, 0);
        terminal.priced = false;
        terminal.checked(true);
        terminal.took_ms = 6 * 60_000;
        let summaries = summarize(&[terminal]);
        let kimi = of(&summaries, "kimi", None, "code").unwrap();
        assert_eq!((kimi.cost_usd, kimi.minutes), (None, Some(6.0)));
        assert!(
            kimi.line().contains("cost not known, 6 min each"),
            "{}",
            kimi.line()
        );
        let old: Outcome =
            serde_json::from_str(r#"{"worker":1,"agent":"codex","job":"code","started_ms":0}"#)
                .unwrap();
        assert!(old.priced, "an outcome kept before was priced");
    }
}
