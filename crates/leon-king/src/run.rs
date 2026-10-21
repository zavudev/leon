//! Runs: a task of the board carried out as its steps, in the order their
//! needs say, without King deciding each start.
//!
//! A [`Run`] is made from a task and its steps (the tasks that name it as
//! their parent) as they were when it started: a step waits until every
//! step it needs is done, then gets a worker of its own, chosen by the
//! router like any other. At most [`Run::max_parallel`] of its steps work at
//! once, and none starts once what its workers cost reaches its budget.
//!
//! **A step's worktree** starts from the branch of a step it needs (with
//! the branches of the others it needs merged in), or from the project's
//! checkout when it needs none: what a step builds on is in its folder. When
//! its worker finishes, Leon commits what it left and runs the project's
//! check there; the step is done once the check passes on that commit. A
//! failed check is told to the same worker to mend, up to
//! [`crate::config::Limits::run_fixes`] times.
//!
//! **King is woken twice at most**: when the run is done (its last steps'
//! branches hold the work, ready for a pull request) and when it cannot go
//! on (a step failed, or the budget is spent). Its steps' own turns are not
//! news; a step's question is, as any worker's is.

use serde::{Deserialize, Serialize};

use crate::coordinator::Job;

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Its steps start as their needs are done.
    Running,
    /// A step failed, or the budget is spent: nothing starts until King or
    /// the person says how to go on.
    Blocked,
    /// Every step is done.
    Done,
    /// It was called off.
    Canceled,
}

/// Where a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// It waits for its needs, or for room to start.
    Waiting,
    /// Its worker works on it.
    Working,
    /// Its worker finished; Leon commits and checks its work.
    Checking,
    /// Its work is committed and checked.
    Done,
    /// It could not start, its worker failed, or its check kept failing.
    Failed,
    /// Its run was called off before it was done.
    Canceled,
}

/// A step as a run is asked for with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSpec {
    /// Its task's id.
    pub task: String,
    /// Its task's title.
    pub title: String,
    /// Its task's body.
    pub body: String,
    /// The ids of the steps it needs done first.
    pub needs: Vec<String>,
    /// The tier its task's labels name, when one does.
    pub tier: Option<String>,
    /// What kind of job it is: `research` when its labels say so.
    pub job: Job,
}

/// A run as it is asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    /// The task it carries out, by id.
    pub task: String,
    /// That task's title.
    pub title: String,
    /// That task's body.
    pub body: String,
    /// The project its steps work in, by name.
    pub project: String,
    /// The steps, in the order of the board.
    pub steps: Vec<StepSpec>,
    /// How many steps work at once, when not the configured number.
    pub max_parallel: Option<usize>,
    /// The most its workers may cost, in dollars.
    pub budget_usd: Option<f64>,
}

/// One step of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Its task's id.
    pub task: String,
    /// Its task's title.
    pub title: String,
    /// Its task's body.
    pub body: String,
    /// The ids of the steps it needs done first.
    pub needs: Vec<String>,
    /// The tier asked for it.
    pub tier: Option<String>,
    /// What kind of job it is.
    pub job: Job,
    /// Where it is.
    pub state: StepState,
    /// Its worker, once it started.
    pub worker: Option<u32>,
    /// The branch it works on, once it started.
    pub branch: Option<String>,
    /// How many times its worker was told to mend a failed check.
    #[serde(default)]
    pub fixes: u32,
    /// Why it failed, or what is to be known of how it was done.
    #[serde(default)]
    pub note: Option<String>,
}

/// A task carried out as its steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Its number, from 1.
    pub id: u32,
    /// The task it carries out, by id.
    pub task: String,
    /// That task's title.
    pub title: String,
    /// That task's body.
    pub body: String,
    /// The project its steps work in, by name.
    pub project: String,
    /// Where it is.
    pub state: RunState,
    /// How many steps work at once.
    pub max_parallel: usize,
    /// The most its workers may cost, in dollars.
    pub budget_usd: Option<f64>,
    /// Its steps, in an order where every step comes after what it needs.
    pub steps: Vec<Step>,
    /// When it started, in milliseconds since the epoch.
    pub started_ms: u64,
    /// When it last changed.
    pub updated_ms: u64,
    /// The conversation of King's it started in.
    #[serde(default)]
    pub started_in: Option<u64>,
    /// Why it is blocked, while it is.
    #[serde(default)]
    pub note: Option<String>,
}

impl Run {
    /// A run of `spec`, numbered `id`. Refused without steps, with a need
    /// that is not one of its steps, or with needs that go round.
    pub fn new(id: u32, spec: RunSpec, max_parallel: usize, now_ms: u64) -> Result<Self, String> {
        if spec.steps.is_empty() {
            return Err(format!(
                "\"{}\" has no steps to run: make its steps tasks whose parent it is \
                 (task_create with parent), with the needs between them",
                spec.title
            ));
        }
        for step in &spec.steps {
            if let Some(need) = step
                .needs
                .iter()
                .find(|need| !spec.steps.iter().any(|other| other.task == **need))
            {
                return Err(format!(
                    "the step \"{}\" needs {need}, which is not a step of \"{}\" that is still \
                     to do",
                    step.title, spec.title
                ));
            }
        }
        // Every step after what it needs, the board's order kept otherwise.
        let mut ordered: Vec<StepSpec> = Vec::new();
        let mut left = spec.steps;
        while !left.is_empty() {
            let at = left
                .iter()
                .position(|step| {
                    step.needs
                        .iter()
                        .all(|need| ordered.iter().any(|done| done.task == *need))
                })
                .ok_or("the needs of these steps go round in a circle")?;
            ordered.push(left.remove(at));
        }
        Ok(Self {
            id,
            task: spec.task,
            title: spec.title,
            body: spec.body,
            project: spec.project,
            state: RunState::Running,
            max_parallel: spec.max_parallel.unwrap_or(max_parallel).max(1),
            budget_usd: spec.budget_usd,
            steps: ordered
                .into_iter()
                .map(|step| Step {
                    task: step.task,
                    title: step.title,
                    body: step.body,
                    needs: step.needs,
                    tier: step.tier,
                    job: step.job,
                    state: StepState::Waiting,
                    worker: None,
                    branch: None,
                    fixes: 0,
                    note: None,
                })
                .collect(),
            started_ms: now_ms,
            updated_ms: now_ms,
            started_in: None,
            note: None,
        })
    }

    /// Whether it is over: done or called off.
    pub fn is_over(&self) -> bool {
        matches!(self.state, RunState::Done | RunState::Canceled)
    }

    /// How many of its steps work now: with a worker, or being checked.
    pub fn active(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step.state, StepState::Working | StepState::Checking))
            .count()
    }

    /// The steps that may start now, by index: waiting, with every need
    /// done, as many as there is room for.
    pub fn ready(&self) -> Vec<usize> {
        if self.state != RunState::Running {
            return Vec::new();
        }
        let room = self.max_parallel.saturating_sub(self.active());
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.state == StepState::Waiting && self.needs_done(step))
            .map(|(index, _)| index)
            .take(room)
            .collect()
    }

    fn needs_done(&self, step: &Step) -> bool {
        step.needs.iter().all(|need| {
            self.step(need)
                .is_some_and(|(_, need)| need.state == StepState::Done)
        })
    }

    /// A step, by its task's id, with its index.
    pub fn step(&self, task: &str) -> Option<(usize, &Step)> {
        self.steps
            .iter()
            .enumerate()
            .find(|(_, step)| step.task == task)
    }

    /// The step a worker works on, by index.
    pub fn step_of(&self, worker: u32) -> Option<usize> {
        self.steps
            .iter()
            .position(|step| step.worker == Some(worker))
    }

    /// The steps that `index` needs, in its order, with their branches.
    pub fn needed_branches(&self, index: usize) -> Vec<String> {
        self.steps[index]
            .needs
            .iter()
            .filter_map(|need| self.step(need))
            .filter_map(|(_, need)| need.branch.clone())
            .collect()
    }

    /// The steps no other step needs: whose branches, together, hold the
    /// work of the whole run.
    pub fn last_steps(&self) -> Vec<&Step> {
        self.steps
            .iter()
            .filter(|step| {
                !self
                    .steps
                    .iter()
                    .any(|other| other.needs.contains(&step.task))
            })
            .collect()
    }

    /// How many of its steps are done.
    pub fn done(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.state == StepState::Done)
            .count()
    }

    /// Where step `index` stands, in a few words: `step 2 of 5`.
    pub fn place(&self, index: usize) -> String {
        format!("step {} of {}", index + 1, self.steps.len())
    }
}

/// What a step's worker is told first.
pub fn step_prompt(run: &Run, index: usize, merge_note: Option<&str>) -> String {
    let step = &run.steps[index];
    let mut text = format!(
        "You carry out one step of a larger task, \"{}\", that Leon runs step by step \
         ({}).\n",
        run.title,
        run.place(index)
    );
    if !run.body.trim().is_empty() {
        text.push_str(&format!("\nThe larger task:\n{}\n", run.body.trim()));
    }
    text.push_str(&format!("\nYour step: {}\n", step.title));
    if !step.body.trim().is_empty() {
        text.push_str(&format!("{}\n", step.body.trim()));
    }
    let built_on: Vec<&str> = step
        .needs
        .iter()
        .filter_map(|need| run.step(need))
        .map(|(_, need)| need.title.as_str())
        .collect();
    if !built_on.is_empty() {
        text.push_str(&format!(
            "\nIt builds on steps done before it, whose work is already in this worktree: {}.\n",
            built_on.join("; ")
        ));
    }
    if let Some(note) = merge_note {
        text.push_str(&format!("\n{note}\n"));
    }
    text.push_str(
        "\nDo only this step, in this worktree. When it is done, say in a few lines what you \
         changed. Leon commits what you leave and runs the project's check on it; you need \
         not push or open a pull request.",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(task: &str, needs: &[&str]) -> StepSpec {
        StepSpec {
            task: task.to_owned(),
            title: format!("Step {task}"),
            body: String::new(),
            needs: needs.iter().map(|need| (*need).to_owned()).collect(),
            tier: None,
            job: Job::Code,
        }
    }

    fn spec(steps: Vec<StepSpec>) -> RunSpec {
        RunSpec {
            task: "goal".to_owned(),
            title: "Ship billing".to_owned(),
            body: String::new(),
            project: "shop".to_owned(),
            steps,
            max_parallel: None,
            budget_usd: None,
        }
    }

    #[test]
    fn steps_start_once_what_they_need_is_done_and_no_more_than_the_room() {
        // The page is listed first, but needs the API, which needs the
        // schema; the docs need nothing.
        let mut run = Run::new(
            1,
            spec(vec![
                step("page", &["api"]),
                step("schema", &[]),
                step("api", &["schema"]),
                step("docs", &[]),
            ]),
            1,
            0,
        )
        .unwrap();
        let order: Vec<&str> = run.steps.iter().map(|step| step.task.as_str()).collect();
        assert_eq!(order, ["schema", "api", "page", "docs"]);
        assert_eq!(run.ready(), [0], "one at a time");
        run.max_parallel = 2;
        assert_eq!(run.ready(), [0, 3]);
        run.steps[0].state = StepState::Working;
        run.steps[3].state = StepState::Working;
        assert!(run.ready().is_empty());
        run.steps[0].state = StepState::Done;
        assert_eq!(run.ready(), [1]);
        run.state = RunState::Blocked;
        assert!(run.ready().is_empty(), "a blocked run starts nothing");
        assert_eq!(
            run.last_steps()
                .iter()
                .map(|step| step.task.as_str())
                .collect::<Vec<_>>(),
            ["page", "docs"]
        );
    }

    #[test]
    fn a_run_needs_steps_whose_needs_are_its_own_and_do_not_go_round() {
        assert!(Run::new(1, spec(Vec::new()), 2, 0).is_err());
        assert!(Run::new(1, spec(vec![step("a", &["elsewhere"])]), 2, 0).is_err());
        assert!(Run::new(1, spec(vec![step("a", &["b"]), step("b", &["a"])]), 2, 0).is_err());
    }

    #[test]
    fn a_step_is_told_the_larger_task_and_what_it_builds_on() {
        let run = Run::new(
            1,
            spec(vec![step("schema", &[]), step("api", &["schema"])]),
            2,
            0,
        )
        .unwrap();
        let prompt = step_prompt(&run, 1, Some("Two branches conflict."));
        assert!(prompt.contains("\"Ship billing\""), "{prompt}");
        assert!(prompt.contains("step 2 of 2"), "{prompt}");
        assert!(prompt.contains("Your step: Step api"), "{prompt}");
        assert!(
            prompt.contains("already in this worktree: Step schema"),
            "{prompt}"
        );
        assert!(prompt.contains("Two branches conflict."), "{prompt}");
    }
}
