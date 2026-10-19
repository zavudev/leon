//! What the coordinator does with runs ([`crate::run`]): which steps start,
//! and what a step's worker finishing, or its check ending, makes of it.

use std::collections::BTreeMap;

use super::{Coordinator, Effect, News, Target, WorkerState};
use crate::run::{Run, RunSpec, RunState, StepState};
use crate::transcript;

/// What a step's worker is told when it is to go on without being told how.
const GO_ON: &str = "Go on with your step and finish it.";

impl Coordinator {
    /// The runs, by number.
    pub fn runs(&self) -> &BTreeMap<u32, Run> {
        &self.state.runs
    }

    /// Starts a run of a task's steps. Refused while another run of the
    /// same task is not over.
    pub fn start_run(&mut self, spec: RunSpec, now_ms: u64) -> Result<(u32, Vec<Effect>), String> {
        if let Some(open) = self
            .state
            .runs
            .values()
            .find(|run| run.task == spec.task && !run.is_over())
        {
            return Err(format!(
                "run #{} of \"{}\" is not over: go on with it (run_continue) or call it off \
                 (run_cancel) first",
                open.id, open.title
            ));
        }
        let id = self
            .state
            .runs
            .keys()
            .next_back()
            .map_or(1, |last| last + 1);
        let mut run = Run::new(id, spec, self.config.limits.run_parallel, now_ms)?;
        run.started_in = Some(self.state.conversation);
        let effects = vec![
            Effect::Changed,
            Effect::Board {
                task: run.task.clone(),
                status: "doing".to_owned(),
            },
        ];
        self.state.runs.insert(id, run);
        Ok((id, effects))
    }

    /// The steps the service is to start now, as `(run, step index)`: those
    /// whose needs are done, as many as each run and the workers' limit
    /// leave room for. A run whose workers spent its budget is blocked
    /// instead, and King hears of it.
    pub fn steps_to_start(&mut self, now_ms: u64) -> (Vec<(u32, usize)>, Vec<Effect>) {
        let mut room = self
            .config
            .limits
            .max_workers
            .saturating_sub(self.working());
        let mut chosen = Vec::new();
        let mut effects = Vec::new();
        let ids: Vec<u32> = self.state.runs.keys().copied().collect();
        for id in ids {
            let ready = self.state.runs[&id].ready();
            if ready.is_empty() {
                continue;
            }
            let spent = self.run_spent(id);
            let run = &self.state.runs[&id];
            if let Some(budget) = run.budget_usd.filter(|budget| spent >= *budget) {
                let why = format!(
                    "its workers spent ${spent:.2} of its ${budget:.2} budget; give it more \
                     (run_continue with budget_usd) or call it off (run_cancel)"
                );
                effects.extend(self.block(id, None, why, now_ms));
                continue;
            }
            for index in ready {
                if room == 0 {
                    break;
                }
                room -= 1;
                chosen.push((id, index));
            }
        }
        (chosen, effects)
    }

    /// What the workers of a run cost so far, in dollars.
    pub fn run_spent(&self, id: u32) -> f64 {
        self.state.runs.get(&id).map_or(0.0, |run| {
            run.steps
                .iter()
                .filter_map(|step| step.worker)
                .filter_map(|worker| self.state.workers.get(&worker))
                .map(|worker| worker.conversation.cost_usd)
                .sum()
        })
    }

    /// A step got its worker, on `branch`.
    pub fn step_started(
        &mut self,
        id: u32,
        index: usize,
        worker: u32,
        branch: &str,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(step) = self
            .state
            .runs
            .get_mut(&id)
            .and_then(|run| run.steps.get_mut(index))
        else {
            return Vec::new();
        };
        step.state = StepState::Working;
        step.worker = Some(worker);
        step.branch = Some(branch.to_owned());
        let task = step.task.clone();
        self.touch(id, now_ms);
        vec![
            Effect::Changed,
            Effect::Board {
                task,
                status: "doing".to_owned(),
            },
        ]
    }

    /// A step could not be given a worker: its run is blocked.
    pub fn step_not_started(
        &mut self,
        id: u32,
        index: usize,
        error: &str,
        now_ms: u64,
    ) -> Vec<Effect> {
        self.fail_step(id, index, format!("could not start: {error}"), now_ms)
    }

    /// The run and step a worker works on.
    pub(super) fn step_of_worker(&self, worker: u32) -> Option<(u32, usize)> {
        self.state.runs.iter().find_map(|(id, run)| {
            run.step_of(worker)
                .filter(|_| !run.is_over())
                .map(|index| (*id, index))
        })
    }

    /// A step's worker ended its turn: its work is to be committed and
    /// checked when the turn went well, and the step failed when not.
    /// `None` when the worker works on no step that waits for it.
    pub(super) fn step_turn_ended(
        &mut self,
        worker: u32,
        ok: bool,
        said: &str,
        now_ms: u64,
    ) -> Option<Vec<Effect>> {
        let (id, index) = self.step_of_worker(worker)?;
        if self.state.runs[&id].steps[index].state != StepState::Working {
            return None;
        }
        if !ok {
            return Some(self.fail_step(id, index, format!("its worker {said}"), now_ms));
        }
        self.state.runs.get_mut(&id)?.steps[index].state = StepState::Checking;
        self.touch(id, now_ms);
        Some(vec![Effect::Changed, Effect::Verify { worker }])
    }

    /// A step's work was committed, and there is no check to run on it: it
    /// is done, with `why` noted.
    pub fn step_unchecked(&mut self, worker: u32, why: &str, now_ms: u64) -> Vec<Effect> {
        match self.checking(worker) {
            Some((id, index)) => self.finish_step(id, index, Some(why.to_owned()), now_ms),
            None => Vec::new(),
        }
    }

    /// A step's work could not be committed or checked.
    pub fn step_not_verified(&mut self, worker: u32, why: &str, now_ms: u64) -> Vec<Effect> {
        match self.checking(worker) {
            Some((id, index)) => self.fail_step(id, index, why.to_owned(), now_ms),
            None => Vec::new(),
        }
    }

    /// A step's check ended: the step is done when it passed; when it
    /// failed, the worker is told to mend it while it has tries left.
    /// `None` when the worker's check was not a step's.
    pub(super) fn step_check_finished(
        &mut self,
        worker: u32,
        passed: bool,
        tail: &str,
        now_ms: u64,
    ) -> Option<Vec<Effect>> {
        let (id, index) = self.checking(worker)?;
        if passed {
            return Some(self.finish_step(id, index, None, now_ms));
        }
        let most = self.config.limits.run_fixes;
        let step = &mut self.state.runs.get_mut(&id)?.steps[index];
        if step.fixes >= most {
            let why = format!(
                "its check failed{}; the end of what it printed:\n{}",
                if most > 0 {
                    format!(" again after {most} tries to mend it")
                } else {
                    String::new()
                },
                transcript::cut(tail, 1_500)
            );
            return Some(self.fail_step(id, index, why, now_ms));
        }
        step.fixes += 1;
        step.state = StepState::Working;
        let text = format!(
            "[Leon] The project's check failed on your work. The end of what it printed:\n{}\n\n\
             Mend it, then say what you changed; Leon checks again.",
            transcript::cut(tail, 3_000)
        );
        self.touch(id, now_ms);
        let mut effects = vec![Effect::Changed];
        if let Some(worker) = self.state.workers.get_mut(&worker) {
            worker.conversation.queue.push_back(text);
        }
        effects.extend(self.next_turn(Target::Worker(worker), now_ms));
        Some(effects)
    }

    /// A step's worker was stopped or taken over while the step was not
    /// done: the step fails.
    pub(super) fn step_left(&mut self, worker: u32, why: &str, now_ms: u64) -> Vec<Effect> {
        match self.step_of_worker(worker) {
            Some((id, index))
                if matches!(
                    self.state.runs[&id].steps[index].state,
                    StepState::Working | StepState::Checking
                ) =>
            {
                self.fail_step(id, index, why.to_owned(), now_ms)
            }
            _ => Vec::new(),
        }
    }

    /// King told a failed step's worker something itself: the step works
    /// again, and the run goes on once no step of it is failed.
    pub(super) fn step_told(&mut self, worker: u32, now_ms: u64) {
        let Some((id, index)) = self.step_of_worker(worker) else {
            return;
        };
        let Some(run) = self.state.runs.get_mut(&id) else {
            return;
        };
        if run.steps[index].state == StepState::Failed {
            run.steps[index].state = StepState::Working;
            run.steps[index].note = None;
            if !run.steps.iter().any(|step| step.state == StepState::Failed) {
                run.state = RunState::Running;
                run.note = None;
            }
            self.touch(id, now_ms);
        }
    }

    /// Goes on with a blocked run: its failed steps' workers are told
    /// `text` (or to go on), a step that never started is tried again, and
    /// `budget_usd` replaces its budget when given.
    pub fn run_continue(
        &mut self,
        id: u32,
        text: Option<&str>,
        budget_usd: Option<f64>,
        now_ms: u64,
    ) -> Result<Vec<Effect>, String> {
        let run = self
            .state
            .runs
            .get_mut(&id)
            .ok_or_else(|| format!("there is no run #{id}"))?;
        if run.is_over() {
            return Err(format!("run #{id} is over"));
        }
        if let Some(budget) = budget_usd {
            run.budget_usd = Some(budget);
        }
        run.state = RunState::Running;
        run.note = None;
        let mut told = Vec::new();
        for step in &mut run.steps {
            if step.state != StepState::Failed {
                continue;
            }
            step.note = None;
            step.fixes = 0;
            match step.worker {
                Some(worker) => {
                    step.state = StepState::Working;
                    told.push(worker);
                }
                None => step.state = StepState::Waiting,
            }
        }
        self.touch(id, now_ms);
        let mut effects = vec![Effect::Changed];
        let text = text.map(str::trim).filter(|text| !text.is_empty());
        for worker in told {
            let gone = self
                .state
                .workers
                .get(&worker)
                .is_none_or(|worker| worker.state == WorkerState::Released);
            if gone {
                // Its worker was taken over: the step starts again.
                if let Some(step) = self
                    .state
                    .runs
                    .get_mut(&id)
                    .and_then(|run| run.steps.iter_mut().find(|s| s.worker == Some(worker)))
                {
                    step.state = StepState::Waiting;
                    step.worker = None;
                }
                continue;
            }
            effects.extend(self.tell_worker(worker, text.unwrap_or(GO_ON), now_ms)?);
        }
        Ok(effects)
    }

    /// Calls a run off: what waits never starts, and the workers of its
    /// steps that work are stopped.
    pub fn run_cancel(&mut self, id: u32, now_ms: u64) -> Result<Vec<Effect>, String> {
        let run = self
            .state
            .runs
            .get_mut(&id)
            .ok_or_else(|| format!("there is no run #{id}"))?;
        if run.is_over() {
            return Err(format!("run #{id} is over"));
        }
        run.state = RunState::Canceled;
        run.note = None;
        let mut working = Vec::new();
        let mut back = Vec::new();
        for step in &mut run.steps {
            match step.state {
                StepState::Done | StepState::Canceled => continue,
                StepState::Working | StepState::Checking => {
                    working.extend(step.worker);
                    back.push(step.task.clone());
                }
                StepState::Waiting | StepState::Failed => {}
            }
            step.state = StepState::Canceled;
        }
        back.push(run.task.clone());
        self.touch(id, now_ms);
        let mut effects = vec![Effect::Changed];
        for worker in working {
            if let Ok(stopped) = self.stop_worker(worker, now_ms) {
                effects.extend(stopped);
            }
        }
        effects.extend(back.into_iter().map(|task| Effect::Board {
            task,
            status: "todo".to_owned(),
        }));
        Ok(effects)
    }

    /// What restarting the service leaves for runs: the steps whose worker
    /// was in a turn go on, and the steps being checked are checked again.
    pub(super) fn steps_restored(&mut self, interrupted: &[u32], now_ms: u64) -> Vec<Effect> {
        let mut effects = Vec::new();
        for worker in interrupted {
            if let Some(conversation) = self
                .state
                .workers
                .get_mut(worker)
                .map(|worker| &mut worker.conversation)
            {
                conversation.queue.push_back(
                    "[Leon] Leon's service restarted while you worked; go on with your step \
                     from where you were."
                        .to_owned(),
                );
            }
            effects.extend(self.next_turn(Target::Worker(*worker), now_ms));
        }
        let checking: Vec<u32> = self
            .state
            .runs
            .values()
            .filter(|run| !run.is_over())
            .flat_map(|run| run.steps.iter())
            .filter(|step| step.state == StepState::Checking)
            .filter_map(|step| step.worker)
            .collect();
        effects.extend(checking.into_iter().map(|worker| Effect::Verify { worker }));
        effects
    }

    /// The run and step of a worker whose step is being checked.
    fn checking(&self, worker: u32) -> Option<(u32, usize)> {
        self.step_of_worker(worker)
            .filter(|(id, index)| self.state.runs[id].steps[*index].state == StepState::Checking)
    }

    fn touch(&mut self, id: u32, now_ms: u64) {
        if let Some(run) = self.state.runs.get_mut(&id) {
            run.updated_ms = now_ms;
        }
    }

    /// A step is done; the run is done with its last one, and King hears
    /// where its work is.
    fn finish_step(
        &mut self,
        id: u32,
        index: usize,
        note: Option<String>,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(run) = self.state.runs.get_mut(&id) else {
            return Vec::new();
        };
        let step = &mut run.steps[index];
        step.state = StepState::Done;
        step.note = note;
        let mut effects = vec![
            Effect::Changed,
            Effect::Board {
                task: step.task.clone(),
                status: "done".to_owned(),
            },
        ];
        run.updated_ms = now_ms;
        if run.done() < run.steps.len() {
            return effects;
        }
        run.state = RunState::Done;
        let unchecked: Vec<&str> = run
            .steps
            .iter()
            .filter(|step| step.note.is_some())
            .map(|step| step.title.as_str())
            .collect();
        let last: Vec<String> = run
            .last_steps()
            .iter()
            .map(|step| {
                format!(
                    "worker #{} on {}",
                    step.worker.unwrap_or_default(),
                    step.branch.as_deref().unwrap_or("its branch")
                )
            })
            .collect();
        let mut text = format!(
            "is done: all {} steps of \"{}\" finished, committed and checked. ",
            run.steps.len(),
            run.title
        );
        if last.len() == 1 {
            text.push_str(&format!(
                "The whole work is on the last step's branch ({}): look at its diff \
                 (worktree_diff) and open its pull request (pr_open with that worker).",
                last[0]
            ));
        } else {
            text.push_str(&format!(
                "Its work ends on {} branches, one per step nothing else needed ({}): open a \
                 pull request for each, or start a worker that brings them together.",
                last.len(),
                last.join("; ")
            ));
        }
        if !unchecked.is_empty() {
            text.push_str(&format!(
                " Not checked, for want of a check: {}.",
                unchecked.join("; ")
            ));
        }
        let worker = run.last_steps().first().and_then(|step| step.worker);
        effects.push(Effect::Board {
            task: run.task.clone(),
            status: "review".to_owned(),
        });
        self.add_news(
            News {
                worker: worker.unwrap_or_default(),
                run: Some(id),
                text,
            },
            now_ms,
        );
        effects.extend(self.tick(now_ms));
        effects
    }

    /// A step failed: its run is blocked, and King hears why.
    fn fail_step(&mut self, id: u32, index: usize, why: String, now_ms: u64) -> Vec<Effect> {
        let Some(step) = self
            .state
            .runs
            .get_mut(&id)
            .and_then(|run| run.steps.get_mut(index))
        else {
            return Vec::new();
        };
        step.state = StepState::Failed;
        step.note = Some(transcript::cut(&why, 600));
        let worker = step.worker;
        self.block(id, worker, why, now_ms)
    }

    /// Blocks a run: nothing more starts, and King hears why.
    fn block(&mut self, id: u32, worker: Option<u32>, why: String, now_ms: u64) -> Vec<Effect> {
        let Some(run) = self.state.runs.get_mut(&id) else {
            return Vec::new();
        };
        let failed = run
            .steps
            .iter()
            .enumerate()
            .find(|(_, step)| step.state == StepState::Failed && step.worker == worker);
        let what = match failed {
            Some((index, step)) => format!("{} (\"{}\") ", run.place(index), step.title),
            None => String::new(),
        };
        let waiting = run
            .steps
            .iter()
            .filter(|step| step.state == StepState::Waiting)
            .count();
        run.state = RunState::Blocked;
        run.note = Some(transcript::cut(&why, 300));
        run.updated_ms = now_ms;
        let text = format!(
            "is blocked, {} of {} steps done: {what}{why}\n{waiting} step(s) wait. Go on with \
             run_continue (tell the worker how, or give a budget) or call it off with \
             run_cancel.",
            run.done(),
            run.steps.len()
        );
        if let Some(worker) = worker {
            self.state.news.retain(|news| news.worker != worker);
        }
        self.add_news(
            News {
                worker: worker.unwrap_or_default(),
                run: Some(id),
                text,
            },
            now_ms,
        );
        let mut effects = vec![Effect::Changed];
        effects.extend(self.tick(now_ms));
        effects
    }
}
