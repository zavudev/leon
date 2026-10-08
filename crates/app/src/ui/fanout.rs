//! One prompt, several agents, in the window.
//!
//! What is made and typed is decided by [`crate::fanout::plan`], a pure
//! function. This file carries it out, following the rules of the window: the
//! engine makes the worktrees (one after the other, through the same path as
//! any worktree, so `worktree_location` applies), the window waits for them to
//! be listed in the local store, and then starts a session in each, behind the
//! project's `setup` command when its `leon.toml` has one
//! ([`Shell::start_worktree_sessions`], which asks about that command once for
//! the whole batch).
//!
//! A problem with one agent never stops the others and is reported by name:
//! an agent that cannot take the prompt or is not installed is left out before
//! anything is made, a worktree git refuses is left out after, and the status
//! line says how many were started and why each of the rest was not. The
//! branches git already has are asked first, so a prompt given again gets
//! numbered branches instead of names git refuses; a `leon.toml` that is wrong
//! is told in the same line as the count, because a later line would have
//! replaced it.

use super::scripts;
use super::shell::Shell;
use crate::engine::StatusKind;
use crate::fanout::{self, Item, Request};
use crate::format;
use crate::launch::{self, Launch};
use gpui_kit::{Context, Window};
use leon_core::{AgentId, ProjectId};
use std::time::Duration;

/// How long to wait for a worktree that was made to be listed.
const LISTED_FOR: Duration = Duration::from_secs(15);

/// How often the list is looked at while waiting.
const LISTED_EVERY: Duration = Duration::from_millis(200);

/// The sentence the status line shows when the work is done: how many agents
/// were started, and the reason for each of the others, by name.
///
/// `notice` is the one more thing to say, such as why no setup was run.
pub fn summary(started: usize, asked: usize, failures: &[String], notice: Option<&str>) -> String {
    let mut text = format!(
        "Started {started} of {asked} agents{}",
        if failures.is_empty() { "." } else { ":" }
    );
    for failure in failures {
        text.push(' ');
        text.push_str(failure);
    }
    if let Some(notice) = notice {
        text.push(' ');
        text.push_str(notice);
    }
    text
}

/// How one agent's problem is told.
pub fn failure(agent: AgentId, why: &str) -> String {
    let why = why.trim_end_matches('.');
    format!("{}: {why}.", format::agent_name(agent))
}

impl Shell {
    /// Makes a worktree for each of `agents` in `project`, from `base`, and
    /// starts each agent in its own with `prompt` on its launch line.
    pub(super) fn start_prompt_agents(
        &mut self,
        project: ProjectId,
        prompt: String,
        agents: Vec<AgentId>,
        base: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The branches the project has, asked of git: a name it has is not
        // one to make again. When git cannot say, it will refuse by name.
        let listed = self.engine.base_refs(project.clone());
        self.fanout_tasks
            .push(cx.spawn_in(window, async move |this, cx| {
                let refs = listed.await.ok().flatten().unwrap_or_default();
                this.update_in(cx, |this, window, cx| {
                    this.plan_prompt_agents(project, prompt, agents, base, &refs, window, cx);
                })
                .ok();
            }));
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_prompt_agents(
        &mut self,
        project: ProjectId,
        prompt: String,
        agents: Vec<AgentId>,
        base: Option<String>,
        refs: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.snapshot.project(&project) else {
            self.engine
                .report(StatusKind::Error, "That project is not known.");
            return;
        };
        let (machine_id, root) = (entry.project.machine_id.clone(), entry.project.root.clone());
        let taken = fanout::taken_branches(
            entry
                .worktrees
                .iter()
                .filter_map(|worktree| worktree.branch.clone()),
            refs,
        );
        let Some(machine) = self.snapshot.machine(&machine_id).cloned() else {
            self.engine
                .report(StatusKind::Error, "That machine is not known.");
            return;
        };
        let prompt = match launch::clean_prompt(&prompt) {
            Ok(prompt) => prompt,
            Err(why) => {
                self.engine.report(StatusKind::Error, why);
                return;
            }
        };
        let prefs = Self::launch_prefs(cx);
        let location = self.engine.prefs().worktree_location;
        let planned = fanout::plan(&Request {
            prompt: &prompt,
            agents: &agents,
            root: &root,
            location: &location,
            taken: &taken,
            flavor: launch::shell_flavor(&machine, &*self.options.system, &prefs),
            prefs: &prefs,
        });
        let plan = match planned {
            Ok(plan) => plan,
            Err(why) => {
                self.engine.report(StatusKind::Error, why);
                return;
            }
        };
        let asked = agents.len();
        let mut failures: Vec<String> = plan
            .left
            .iter()
            .map(|left| failure(left.agent, &left.why))
            .collect();
        // What the machine can tell at once: is the agent installed there.
        let report = match self.engine.machine_state(&machine_id) {
            crate::engine::MachineState::Online(Some(report)) => Some(report),
            _ => None,
        };
        let mut items: Vec<Item> = Vec::new();
        for item in plan.items {
            let launch = Launch::Prompted {
                kind: item.agent,
                prompt: prompt.clone(),
                account: super::terminals::default_account(item.agent, cx),
            };
            match launch::plan_with(
                &machine,
                report.as_ref(),
                &root,
                &launch,
                &self.engine.ssh(),
                &*self.options.system,
                &prefs,
            ) {
                Ok(_) => items.push(item),
                Err(why) => failures.push(failure(item.agent, &why.to_string())),
            }
        }
        if items.is_empty() {
            self.engine
                .report(StatusKind::Error, summary(0, asked, &failures, None));
            return;
        }
        // The fan-out never asks: each agent starts as the account the
        // settings name for it (`default_accounts`), else as its own setup.
        let chosen: Vec<(AgentId, Option<String>)> = items
            .iter()
            .map(|item| {
                (
                    item.agent,
                    super::terminals::default_account(item.agent, cx),
                )
            })
            .collect();
        let branches = items.iter().map(|item| item.branch.clone()).collect();
        let made = self.engine.add_worktrees(project.clone(), branches, base);
        let engine = self.engine.clone();
        self.fanout_tasks
            .push(cx.spawn_in(window, async move |this, cx| {
                let mut results = made.await.unwrap_or_default().into_iter();
                let mut ready = Vec::new();
                for item in items {
                    match results.next() {
                        Some(Ok(_)) => ready.push(item),
                        Some(Err(why)) => failures.push(failure(item.agent, &why.to_string())),
                        None => failures.push(failure(item.agent, "the work did not finish")),
                    }
                }
                // The window reads the local store: the sessions start where
                // the store says the worktrees are.
                let mut starts: Vec<(String, Launch)> = Vec::new();
                for item in ready {
                    let mut waited = Duration::ZERO;
                    let path = loop {
                        let found = this.read_with(cx, |this, _| {
                            this.snapshot.project(&project).and_then(|entry| {
                                entry
                                    .worktrees
                                    .iter()
                                    .find(|worktree| {
                                        worktree.branch.as_deref() == Some(item.branch.as_str())
                                    })
                                    .map(|worktree| worktree.path.clone())
                            })
                        });
                        match found {
                            Ok(Some(path)) => break Some(path),
                            Ok(None) if waited < LISTED_FOR => {
                                cx.background_executor().timer(LISTED_EVERY).await;
                                waited += LISTED_EVERY;
                            }
                            _ => break None,
                        }
                    };
                    match path {
                        Some(path) => starts.push((
                            path,
                            Launch::Prompted {
                                kind: item.agent,
                                prompt: prompt.clone(),
                                account: chosen
                                    .iter()
                                    .find(|(agent, _)| *agent == item.agent)
                                    .and_then(|(_, account)| account.clone()),
                            },
                        )),
                        None => failures.push(failure(
                            item.agent,
                            "its worktree was made but is not in the list yet",
                        )),
                    }
                }
                if starts.is_empty() {
                    this.update(cx, |this, _| {
                        this.engine
                            .report(StatusKind::Error, summary(0, asked, &failures, None));
                    })
                    .ok();
                    return;
                }
                // The project's file is read now: the setup it names is the
                // one that runs, not one from before an edit.
                let state = engine
                    .read_project_file(project.clone())
                    .await
                    .ok()
                    .and_then(Result::ok);
                this.update_in(cx, |this, window, cx| {
                    let notice = scripts::setup_notice(&state);
                    let kind = if failures.is_empty() && notice.is_none() {
                        StatusKind::Info
                    } else {
                        StatusKind::Error
                    };
                    this.engine.report(
                        kind,
                        summary(starts.len(), asked, &failures, notice.as_deref()),
                    );
                    this.start_worktree_sessions(&project, &machine_id, starts, state, window, cx);
                })
                .ok();
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_names_each_agent_that_was_not_started() {
        assert_eq!(summary(3, 3, &[], None), "Started 3 of 3 agents.");
        assert_eq!(
            summary(
                1,
                3,
                &[
                    failure(AgentId::CODEX, "git refused the branch."),
                    failure(AgentId::CLAUDE, "it is not installed"),
                ],
                None
            ),
            "Started 1 of 3 agents: Codex: git refused the branch. Claude Code: it is not installed."
        );
        assert_eq!(summary(0, 2, &[], None), "Started 0 of 2 agents.");
    }

    #[test]
    fn a_notice_about_the_setup_stays_in_the_line_with_the_count() {
        let notice = scripts::setup_notice(&Some(crate::project::ProjectState::Invalid(
            crate::project::ProjectError {
                line: 3,
                message: "unknown field `run`".into(),
            },
        )))
        .unwrap();
        assert_eq!(
            summary(2, 2, &[], Some(&notice)),
            "Started 2 of 2 agents. No setup was run: leon.toml line 3: unknown field `run`"
        );
        assert!(scripts::setup_notice(&None)
            .unwrap()
            .contains("could not read leon.toml"));
        assert_eq!(
            scripts::setup_notice(&Some(crate::project::ProjectState::Absent)),
            None
        );
    }
}
