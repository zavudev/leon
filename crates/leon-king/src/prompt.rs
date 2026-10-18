//! What King and its workers are told beside what they are asked.
//!
//! * [`king`]: King's instructions, written by the service into the
//!   instruction files of King's folder (`AGENTS.md`, `CLAUDE.md`), so that
//!   whatever agent King runs on reads them: its role, what it may and may
//!   not do, how it saves, and the projects Leon knows;
//! * [`worker`]: what a worker is told beside its prompt, on every turn;
//! * [`wake`]: the message that gives King the news of its workers.
//!
//! Pure: text in, text out.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::config::Config;
use crate::coordinator::{Job, News, Worker};

/// A project as King's instructions list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLine {
    /// Its name.
    pub name: String,
    /// The machine it is on.
    pub machine: String,
    /// Its root folder there.
    pub root: String,
    /// Its worktrees' branches, the main one first.
    pub branches: Vec<String>,
}

/// King's instructions.
pub fn king(config: &Config, projects: &[ProjectLine]) -> String {
    let mut text = String::from(
        "# King Leon\n\n\
         You are King Leon, the principal engineer of the person you talk to. You answer for \
         every project Leon knows (listed below), on every machine. You are their one \
         interlocutor: they ask you anything (how something works, what state a project is \
         in, what to build next, to build it) and you answer, decide and get it done.\n\n\
         ## How you work\n\n\
         * Answer a short question yourself: read the code (your tools `read_file`, \
           `search_code`, `project_status`, or your own when the project is on this computer).\n\
         * Give a long investigation or any change of code to a **worker** \
           (`worker_start`): another agent, in a worktree of its own. A `research` job reads \
           and reports; a `code` job changes code and commits on its branch. Pick the \
           cheapest tier that will do the job well; `usage_status` says what each agent has \
           left. Then summarise for the person what the worker found or did.\n\
         * You are woken with news of your workers when they finish, fail or ask something: \
           do not wait or poll for them. Answer their questions with `worker_answer`, tell \
           them more with `worker_send`, read what they did with `worker_transcript`.\n\
         * Check a worker's work before you call it done: read its diff (`worktree_diff`), \
           run the project's checks through a worker if they are long.\n\
         * Keep what should outlive this conversation (decisions, conventions, what you \
           learnt of a project) in Leon's shared memory (`memory_add`), and read it \
           (`memory_search`) before you decide something you may have decided before.\n\
         * Tell the person with `notify_user` when something they asked for is done or \
           needs their decision, if they may not be watching.\n\n\
         ## What you may do\n\n\
         * Create worktrees and start, steer and stop workers.\n\
         * Commit on a worktree's branch, push it and open a pull request (`pr_open`).\n\n\
         ## What you never do\n\n\
         * Merge a pull request, push to a project's default branch, or force a push. Those \
           are the person's. Leon refuses them anyway.\n\
         * Work in a project's main checkout: every change happens in a worktree.\n\
         * Spend past the day's budget: when Leon refuses a worker for it, ask the person.\n",
    );
    let _ = writeln!(text, "\n## Tiers of workers\n");
    for (name, candidates) in &config.tiers {
        let names: Vec<String> = candidates
            .iter()
            .map(|candidate| match &candidate.model {
                Some(model) => format!("{} ({model})", candidate.agent),
                None => candidate.agent.clone(),
            })
            .collect();
        let _ = writeln!(text, "* `{name}`: {}", names.join(", then "));
    }
    let _ = writeln!(
        text,
        "\nAt most {} workers work at the same time{}.",
        config.limits.max_workers,
        match config.limits.daily_budget_usd {
            Some(budget) => format!(", and they may cost ${budget:.2} a day"),
            None => String::new(),
        }
    );
    let _ = writeln!(text, "\n## Projects\n");
    if projects.is_empty() {
        let _ = writeln!(text, "Leon knows no project yet.");
    }
    for project in projects {
        let _ = writeln!(
            text,
            "* **{}** on {}: `{}`{}",
            project.name,
            project.machine,
            project.root,
            match project.branches.as_slice() {
                [] => String::new(),
                branches => format!(" (branches: {})", branches.join(", ")),
            }
        );
    }
    text
}

/// What a worker is told beside its prompt.
pub fn worker(worker: &Worker) -> String {
    let job = match worker.job {
        Job::Code => {
            "Change what the task needs in this folder only, run the project's checks, and \
             commit on the current branch with a Conventional Commit message when the work is \
             done. Do not push, do not merge, do not switch branches."
        }
        Job::Research => {
            "Read and investigate; change nothing, commit nothing. End with a clear, \
             complete report: what you found, where (files and lines), and what you would do."
        }
    };
    format!(
        "You are worker #{} of King Leon, a coordinating agent, on the task \"{}\". Your folder \
         is {}. {job} When you need a decision you cannot make, ask it as a question and stop; \
         King will answer.",
        worker.id, worker.title, worker.cwd
    )
}

/// The message that gives King the news of its workers.
pub fn wake(news: &[News], workers: &BTreeMap<u32, Worker>) -> String {
    let mut text = String::from("[Leon] News of your workers:\n");
    for item in news {
        let who = match workers.get(&item.worker) {
            Some(worker) => format!(
                "#{} \"{}\" ({}{})",
                worker.id,
                worker.title,
                worker.conversation.agent,
                worker
                    .conversation
                    .model
                    .as_deref()
                    .map(|model| format!(", {model}"))
                    .unwrap_or_default()
            ),
            None => format!("#{}", item.worker),
        };
        let _ = writeln!(text, "\n- {who} {}", item.text);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kings_instructions_name_its_limits_tiers_and_projects() {
        let mut config = Config::default();
        config.limits.daily_budget_usd = Some(20.0);
        let text = king(
            &config,
            &[ProjectLine {
                name: "leon".into(),
                machine: "This computer".into(),
                root: "/code/leon".into(),
                branches: vec!["main".into(), "feature/x".into()],
            }],
        );
        assert!(text.contains("never do"));
        assert!(text.contains("Merge a pull request"));
        assert!(text.contains("* `cheap`: opencode, then cursor"));
        assert!(text.contains("$20.00 a day"));
        assert!(
            text.contains("**leon** on This computer: `/code/leon` (branches: main, feature/x)")
        );
        assert!(king(&Config::default(), &[]).contains("knows no project yet"));
    }
}
