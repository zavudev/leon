//! What the service does for runs ([`leon_king::run`]): it reads a task's
//! steps from the board, gives each step that may start a worktree and a
//! worker, commits and checks a step's work when its worker is done, and
//! moves the steps' tasks across the board as they go.

use leon_core::store::tasks::MAX_LISTED;
use leon_core::{MemoryScope, TaskFilter, TaskStatus};
use leon_king::coordinator::{Effect, Job, WorkerSpec};
use leon_king::route::{self, Ask};
use leon_king::run::{self, RunSpec, StepSpec};
use serde_json::{json, Value};

use super::super::place::Place;
use super::{add_worktree_from, now_ms, slug, Service};

impl Service {
    /// Starts a run of a task: its open steps, with their needs.
    pub(super) fn start_run(
        &mut self,
        task: &str,
        max_parallel: Option<usize>,
        budget_usd: Option<f64>,
        now: u64,
    ) -> Result<(Value, Vec<Effect>), String> {
        let store = self
            .store
            .clone()
            .ok_or("Leon's database could not be opened")?;
        let said = |error: leon_core::StoreError| crate::tasks::sentence(&error);
        let goal = store.task(task).map_err(said)?;
        let MemoryScope::Project(root) = &goal.scope else {
            return Err(
                "that task is on the global board, which is no project's; a run works in a \
                 project"
                    .to_owned(),
            );
        };
        let project = self
            .projects()
            .into_iter()
            .find(|project| {
                project.root == *root
                    || project
                        .worktrees
                        .iter()
                        .any(|worktree| worktree["path"].as_str() == Some(root.as_str()))
            })
            .ok_or_else(|| format!("Leon knows no project at {root}"))?;
        if !project.local {
            return Err(format!(
                "{} is on {}; workers run on this computer for now",
                project.name, project.machine
            ));
        }
        let filter = TaskFilter {
            parent: Some(goal.id.clone()),
            ..TaskFilter::default()
        };
        let children = store
            .tasks(&goal.scope, &filter, MAX_LISTED)
            .map_err(said)?;
        let open: Vec<&String> = children
            .iter()
            .filter(|child| !child.status.is_closed())
            .map(|child| &child.id)
            .collect();
        let tiers = &self.coordinator.config().tiers;
        let mut steps = Vec::new();
        for child in children.iter().filter(|child| !child.status.is_closed()) {
            // A need already closed, here or elsewhere, is met.
            let needs = child
                .needs
                .iter()
                .filter(|need| {
                    open.contains(need)
                        || store
                            .task(need)
                            .map(|need| !need.status.is_closed())
                            .unwrap_or(false)
                })
                .cloned()
                .collect();
            steps.push(StepSpec {
                task: child.id.clone(),
                title: child.title.clone(),
                body: child.body.clone(),
                needs,
                tier: child
                    .labels
                    .iter()
                    .find(|label| tiers.contains_key(label.as_str()))
                    .cloned(),
                job: if child.labels.iter().any(|label| label == "research") {
                    Job::Research
                } else {
                    Job::Code
                },
            });
        }
        let spec = RunSpec {
            task: goal.id.clone(),
            title: goal.title.clone(),
            body: goal.body.clone(),
            project: project.name.clone(),
            steps,
            max_parallel,
            budget_usd,
        };
        let (id, effects) = self.coordinator.start_run(spec, now)?;
        let run = &self.coordinator.runs()[&id];
        Ok((
            json!({
                "run": id,
                "project": run.project,
                "steps": run
                    .steps
                    .iter()
                    .map(|step| json!({
                        "task": step.task,
                        "title": step.title,
                        "needs": step.needs,
                        "tier": step.tier,
                    }))
                    .collect::<Vec<_>>(),
                "max_parallel": run.max_parallel,
                "budget_usd": run.budget_usd,
                "note": "Leon starts each step once what it needs is done and checked. You \
                         hear when the run is done, or when it cannot go on; not of each step.",
            }),
            effects,
        ))
    }

    /// Starts the steps of runs that may start now.
    pub(super) fn advance_runs(&mut self) {
        let now = now_ms();
        let (ready, mut effects) = self.coordinator.steps_to_start(now);
        for (run, index) in ready {
            effects.extend(match self.start_step(run, index, now) {
                Ok(effects) => effects,
                Err(error) => self.coordinator.step_not_started(run, index, &error, now),
            });
        }
        if !effects.is_empty() {
            self.carry_out(effects);
        }
    }

    /// Gives a step its worktree and its worker.
    fn start_step(&mut self, id: u32, index: usize, now: u64) -> Result<Vec<Effect>, String> {
        let run = self
            .coordinator
            .runs()
            .get(&id)
            .cloned()
            .ok_or("the run is gone")?;
        let step = &run.steps[index];
        let project = self
            .projects()
            .into_iter()
            .find(|project| project.name == run.project)
            .ok_or_else(|| format!("Leon knows no project {:?} any more", run.project))?;
        let ask = Ask {
            tier: step.tier.clone(),
            agent: None,
            model: None,
            effort: None,
            job: step.job.as_str().to_owned(),
        };
        let standings = self.standings(now);
        let choice = route::route(
            self.coordinator.config(),
            &ask,
            &standings,
            &self.coordinator.record(),
            self.coordinator.spent_today(now),
        )?;
        let branch = format!("king/run{id}-{}", slug(&step.title));
        let needed = run.needed_branches(index);
        let machine = (!project.local).then(|| project.machine_id.clone());
        let place = self.place(machine.as_deref())?;
        let path = add_worktree_from(
            &place,
            &project.root,
            &branch,
            &self.coordinator.config().guards.protected_branches,
            needed.first().map(String::as_str),
        )?;
        let merged = merge_into(&place, &path, needed.get(1..).unwrap_or_default());
        let spec = WorkerSpec {
            title: step.title.clone(),
            prompt: run::step_prompt(&run, index, merged.as_deref()),
            cwd: path,
            machine,
            terminal: false,
            project: Some(project.name.clone()),
            job: step.job,
            choice,
            mode: None,
        };
        let (worker, mut effects) = self.coordinator.start_worker(spec, now)?;
        effects.extend(
            self.coordinator
                .step_started(id, index, worker, &branch, now),
        );
        Ok(effects)
    }

    /// Commits what a step's worker left, then checks it.
    pub(super) fn verify_step(&mut self, worker: u32) -> Vec<Effect> {
        let now = now_ms();
        let Some(found) = self.coordinator.worker(worker) else {
            return Vec::new();
        };
        let (cwd, title, project) = (
            found.cwd.clone(),
            found.title.clone(),
            found.project.clone(),
        );
        let place = match self.worker_place(worker) {
            Ok(place) => place,
            Err(error) => return self.coordinator.step_not_verified(worker, &error, now),
        };
        if let Err(error) = super::super::checks::commit_all(&place, &cwd, &title) {
            return self.coordinator.step_not_verified(
                worker,
                &format!("its work could not be committed: {error}"),
                now,
            );
        }
        if let Err(why) = super::super::checks::command_for(
            &self.coordinator.config().checks,
            project.as_deref(),
            &place,
            &cwd,
        ) {
            return self.coordinator.step_unchecked(worker, &why, now);
        }
        match self.start_check(worker, now) {
            Ok((_, effects)) => effects,
            Err(error) => self.coordinator.step_not_verified(
                worker,
                &format!("its check could not start: {error}"),
                now,
            ),
        }
    }

    /// Moves a task of the board to a column, unless it is there already.
    pub(super) fn move_task(&mut self, task: &str, status: &str) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let Ok(status) = TaskStatus::parse(status) else {
            return;
        };
        match store.task(task) {
            Ok(found) if found.status == status => {}
            Ok(_) => {
                let at = i64::try_from(now_ms()).unwrap_or(i64::MAX);
                if let Err(error) = store.move_task(task, status, None, at) {
                    tracing::warn!(%error, task, "a run's task could not be moved");
                }
            }
            Err(error) => tracing::warn!(%error, task, "a run's task is gone"),
        }
    }
}

/// Merges `branches` into the worktree at `path`, one after the other: what
/// a step needs besides the branch its own starts from. What could not be
/// merged cleanly is said to the step's worker, which resolves it.
fn merge_into(place: &Place, path: &str, branches: &[String]) -> Option<String> {
    let git = |args: &[&str]| place.run("git", args, path);
    for (at, branch) in branches.iter().enumerate() {
        if git(&["merge", "--no-edit", branch.as_str()]).is_ok() {
            continue;
        }
        let conflicted = git(&["diff", "--name-only", "--diff-filter=U"])
            .map(|text| text.trim().to_owned())
            .unwrap_or_default();
        let rest = &branches[at + 1..];
        let mut note = if conflicted.is_empty() {
            let _ = git(&["merge", "--abort"]);
            format!(
                "Leon could not merge {branch}, which holds the work of a step this one needs, \
                 into this worktree: merge it first (git merge --no-edit {branch})."
            )
        } else {
            format!(
                "The work of the steps this one needs was merged into this worktree, and \
                 {branch} conflicts in: {}. Resolve the conflict first, keeping the work of \
                 both, and commit the merge.",
                conflicted.lines().collect::<Vec<_>>().join(", ")
            )
        };
        if !rest.is_empty() {
            note.push_str(&format!(
                " Then merge what is not merged yet: {}.",
                rest.join(", ")
            ));
        }
        return Some(note);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(cwd: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
    }

    #[test]
    fn the_branches_a_step_needs_are_merged_in_and_a_conflict_is_left_to_its_worker() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "base\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "base"]);
        for (branch, file, text) in [
            ("one", "one.txt", "one\n"),
            ("two", "two.txt", "two\n"),
            ("three", "a.txt", "three\n"),
            ("four", "a.txt", "four\n"),
        ] {
            git(root, &["checkout", "-qb", branch, "main"]);
            std::fs::write(root.join(file), text).unwrap();
            git(root, &["add", file]);
            git(root, &["commit", "-qm", branch]);
        }
        git(root, &["checkout", "-q", "one"]);
        git(root, &["config", "user.name", "t"]);
        git(root, &["config", "user.email", "t@t"]);
        let path = root.to_str().unwrap();
        assert_eq!(merge_into(&Place::here(), path, &["two".to_owned()]), None);
        assert!(root.join("two.txt").exists());
        let note = merge_into(
            &Place::here(),
            path,
            &["three".to_owned(), "four".to_owned()],
        )
        .unwrap();
        assert!(note.contains("four conflicts in: a.txt"), "{note}");
    }
}
