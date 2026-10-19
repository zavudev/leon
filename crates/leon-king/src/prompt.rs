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

/// What King's instructions say of the moment they are written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Now<'a> {
    /// The projects Leon knows.
    pub projects: &'a [ProjectLine],
    /// The models of this computer and what is known of each
    /// ([`crate::models::render`]).
    pub models: &'a str,
    /// What was used and what is left ([`crate::report::line`]).
    pub usage: &'a str,
}

/// King's instructions.
pub fn king(config: &Config, now: &Now<'_>) -> String {
    let projects = now.projects;
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
           them more with `worker_send`, read what they did with `worker_transcript`. \
           Answer a permission with `allow`, `always` or `deny` (words, never ids like `d.2`): \
           a denial ends a Codex worker's turn. Give `always` to what a worker does in its own \
           worktree.\n\
         * A worker saying it is done is not evidence. Before you call its work done, read \
           its diff (`worktree_diff`) and run the project's own check on its last commit \
           (`worktree_check`, which answers at once; you hear how it ended as news). When it \
           fails, send the worker the end of what it printed to mend, and check again. Tell \
           the person what was checked, on which commit, and how it ended.\n\
         * `pr_open` needs that passed check, and writes it in the description. Its CI is \
           then watched: you hear when it passes (the person is told it is theirs to \
           review) or which checks fail (have the worker mend them on the same branch, \
           check, and `pr_open` again to push).\n\
         * Keep what should outlive this conversation (decisions, conventions, what you \
           learnt of a project) in Leon's shared memory (`memory_add`), and read it \
           (`memory_search`) before you decide something you may have decided before.\n\
         * Tell the person with `notify_user` when something they asked for is done or \
           needs their decision, if they may not be watching.\n\n\
         ## Tasks\n\n\
         * Each project has a board of tasks the person uses too (Leon's `task_*` tools). \
           Put on it what is found and not done now (a bug, a follow-up, the steps of a \
           plan): `task_create` with `project` set to the project's root folder, as \
           `projects_list` gives it. Without `project` a task lands on the global board.\n\
         * The person may send you a task from the board (\"Task <id> of <project>\"). Work \
           it as any request: move it to `doing` (`task_move`), give it to a worker, set its \
           `branch` (`task_update`) once a worktree has one, move it to `review` when its pull \
           request is open, and say on the task (its body) what was done. Never move a task \
           to `done`: the person does, after the merge.\n\n\
         ## Choosing a model\n\n\
         * Every model you can use is listed below, with its talents (1 to 5) and the \
           benchmarks known of it, each with its source and date. Choose by the job: the \
           talent it needs, then the price (economy) and what the agent has left \
           (`usage_status`). Name the agent and model in `worker_start` when a tier would \
           not pick the right one.\n\
         * **Planning goes to the best models.** Any plan that matters (a feature, a \
           refactor, an architecture, an order of work across projects) is a `plan` job, \
           which runs where \"Kinds of jobs\" below says. Plan yourself only if you run on \
           one of that tier's models; otherwise start a `plan` worker and build on its plan. \
           Execution can then go to cheaper workers, step by step, and the best model \
           reviews what they did.\n\
         * When you tell the person which worker does a job, always name the agent **and \
           the model** (and the effort when one was set): \"codex, gpt-5.6-sol, high\", never \
           \"codex\" alone; an agent has many models. When `worker_start` says the worker runs \
           on its agent's default, read the model it started with in `king_status` before \
           you name it.\n\
         * When you learn something about a model (a benchmark with its source, how it did \
           on a job of ours), keep it with `model_note`, so the next choice is better.\n\n\
         ## Reporting\n\n\
         * Leon writes, under every answer of yours, the tokens it took, how fast, what it \
           cost, what was spent today and what is left of each agent's usage. Use those \
           numbers when you advise the person (what to run now, what can wait for a reset, \
           which agent to save), and say when a job will use a large share of what is left.\n\n\
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
    let _ = writeln!(text, "\n## Kinds of jobs\n");
    for (job, rule) in &config.jobs {
        let _ = writeln!(
            text,
            "* `{job}` goes to the `{}` tier{}",
            rule.tier,
            if rule.strict {
                ", and only ever to its models: Leon refuses any other for it"
            } else {
                " unless you name another tier or an agent"
            }
        );
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
    let _ = writeln!(text, "\n## Models you can use\n");
    let _ = write!(text, "{}", now.models);
    if !now.usage.is_empty() {
        let _ = writeln!(text, "\n## Usage now\n\n{}", now.usage);
    }
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
        Job::Plan => {
            "Plan only; change nothing, commit nothing. Read what the task touches, then \
             write the plan: the approach and why, the steps in order with the files each \
             one changes, the risks, and how to verify the result. Say what you are unsure \
             of instead of guessing."
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
        let projects = [ProjectLine {
            name: "leon".into(),
            machine: "This computer".into(),
            root: "/code/leon".into(),
            branches: vec!["main".into(), "feature/x".into()],
        }];
        let text = king(
            &config,
            &Now {
                projects: &projects,
                models: "* **claude**\n  * `opus` (Opus)\n",
                usage: "Left: claude: 5h 66% left",
            },
        );
        assert!(text.contains("Planning goes to the best models"));
        assert!(text.contains("* `plan` goes to the `planning` tier, and only ever to its models"));
        assert!(text.contains("* `code` goes to the `standard` tier unless you name"));
        assert!(text.contains("## Models you can use\n\n* **claude**"));
        assert!(text.contains("## Usage now\n\nLeft: claude: 5h 66% left"));
        assert!(text.contains("never do"));
        assert!(text.contains("Merge a pull request"));
        assert!(text.contains("* `cheap`: opencode, then cursor"));
        assert!(text.contains("$20.00 a day"));
        assert!(
            text.contains("**leon** on This computer: `/code/leon` (branches: main, feature/x)")
        );
        assert!(king(&Config::default(), &Now::default()).contains("knows no project yet"));
    }
}
