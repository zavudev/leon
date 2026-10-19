//! Syncing tasks with the issues of a repository, decided without a process.
//!
//! [`plan`] takes what the store holds (the tasks of one project and the links
//! between tasks and issues) and what GitHub says (its issues) and answers
//! what is to be done, as [`Step`]s the engine carries out. It does not touch
//! the network, the store or the clock.
//!
//! **A three-way merge.** Every link keeps a base: what the task and the issue
//! said when they were last made equal. A field changed on one side only goes
//! to the other; a field changed on both is decided by which was changed
//! last; the labels are merged as sets, an addition or a removal on either
//! side surviving. Nothing is read from a timestamp to find out *whether* a
//! side changed (the sync's own writes move them): only to break a tie.
//!
//! **The mapping.**
//!
//! | Task | Issue |
//! | --- | --- |
//! | done | closed |
//! | canceled | closed as not planned |
//! | backlog, todo, doing, review | open, with the label `status:<column>` |
//! | priority low to urgent | the label `priority:<level>` |
//! | other labels | the same labels, lower case |
//! | title, body | the same, cleaned as the store cleans them |
//! | assignee | pulled only, and only into an empty one: agents are not GitHub users |
//!
//! An open issue with no status label is `todo`, unless its task was closed
//! (then it was reopened, and is `todo`) or open (then the label was only
//! removed, and the column stays).
//!
//! **What it never does.** It never makes an issue from a task (that is the
//! person's, one card at a time), never deletes an issue, never imports a
//! closed issue, never imports an issue whose link is a tombstone, and never
//! writes back a body that was cut to fit a task.
//!
//! **Idempotent.** A task and an issue that agree make no step, whatever the
//! spelling of their text (`\r\n`, case) and their timestamps: syncing twice
//! with no change in between writes nothing on either side.

use std::collections::BTreeSet;

use leon_core::store::tasks::{MAX_BODY_CHARS, MAX_LABELS, MAX_LABEL_CHARS, MAX_TITLE_CHARS};
use leon_core::{SyncBase, Task, TaskLink, TaskPatch, TaskPriority, TaskStatus};
use leon_remote::{Issue, IssueEdit};

use crate::task_board::{clean_body, clean_line};

/// The prefix of the label that says a task's column.
pub const STATUS_PREFIX: &str = "status:";
/// The prefix of the label that says a task's priority.
pub const PRIORITY_PREFIX: &str = "priority:";

/// The labels a synced repository needs, with a colour and a description:
/// made when sync is turned on, so that no later call names a label that is
/// not there.
pub fn marker_labels() -> Vec<(String, &'static str, String)> {
    let mut labels = Vec::new();
    for (status, colour) in [
        (TaskStatus::Backlog, "c5def5"),
        (TaskStatus::Todo, "bfdadc"),
        (TaskStatus::Doing, "fbca04"),
        (TaskStatus::Review, "d4c5f9"),
    ] {
        labels.push((
            status_label(status).unwrap_or_default(),
            colour,
            format!("Column of the task board: {}", status.as_str()),
        ));
    }
    for (priority, colour) in [
        (TaskPriority::Low, "e4e669"),
        (TaskPriority::Medium, "fef2c0"),
        (TaskPriority::High, "f9d0c4"),
        (TaskPriority::Urgent, "b60205"),
    ] {
        labels.push((
            priority_label(priority).unwrap_or_default(),
            colour,
            format!("Priority of the task board: {}", priority.as_str()),
        ));
    }
    labels
}

/// The label of a status, for the ones an open issue wears.
pub fn status_label(status: TaskStatus) -> Option<String> {
    (!status.is_closed()).then(|| format!("{STATUS_PREFIX}{}", status.as_str()))
}

/// The label of a priority, when it has one.
pub fn priority_label(priority: TaskPriority) -> Option<String> {
    (priority != TaskPriority::None).then(|| format!("{PRIORITY_PREFIX}{}", priority.as_str()))
}

/// What the issue says of a task, in a task's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// The title, cleaned.
    pub title: String,
    /// The body, cleaned.
    pub body: String,
    /// The column.
    pub status: TaskStatus,
    /// The priority.
    pub priority: TaskPriority,
    /// The labels other than the markers, lower case.
    pub labels: BTreeSet<String>,
}

/// An issue in a task's terms. `base` is the status the task had at the last
/// sync, which decides an open issue that has no status label.
pub fn said_by(issue: &Issue, base: Option<TaskStatus>) -> Said {
    let mut status_word: Option<TaskStatus> = None;
    let mut priority = TaskPriority::None;
    let mut labels = BTreeSet::new();
    for label in &issue.labels {
        let lower: String = clean_line(label)
            .to_lowercase()
            .chars()
            .take(MAX_LABEL_CHARS + 20)
            .collect();
        if let Some(word) = lower.strip_prefix(STATUS_PREFIX) {
            if let Ok(status) = TaskStatus::parse(word) {
                if !status.is_closed() {
                    status_word = Some(status);
                }
            }
        } else if let Some(word) = lower.strip_prefix(PRIORITY_PREFIX) {
            if let Ok(found) = TaskPriority::parse(word) {
                priority = found;
            }
        } else if !lower.is_empty() {
            labels.insert(lower);
        }
    }
    let status = if !issue.open {
        if issue.not_planned {
            TaskStatus::Canceled
        } else {
            TaskStatus::Done
        }
    } else {
        match (status_word, base) {
            (Some(status), _) => status,
            (None, Some(base)) if !base.is_closed() => base,
            (None, _) => TaskStatus::Todo,
        }
    };
    // What a task cannot hold is not in what the issue says either: a
    // title is cut at a task's longest, and the labels are the first ones a
    // task takes (they are sorted, as a task is given them). The rest is
    // never compared, so it is never pushed away.
    let labels = labels
        .into_iter()
        .map(|label| label.chars().take(MAX_LABEL_CHARS).collect::<String>())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_LABELS)
        .collect();
    Said {
        title: clean_line(&issue.title)
            .chars()
            .take(MAX_TITLE_CHARS)
            .collect(),
        body: clean_body(&issue.body),
        status,
        priority,
        labels,
    }
}

/// A task in the same terms.
pub fn said_by_task(task: &Task) -> Said {
    Said {
        title: clean_line(&task.title),
        body: clean_body(&task.body),
        status: task.status,
        priority: task.priority,
        labels: task
            .labels
            .iter()
            .map(|label| clean_line(label).to_lowercase())
            .filter(|label| !label.is_empty())
            .collect(),
    }
}

fn said_by_base(base: &SyncBase) -> Said {
    Said {
        title: clean_line(&base.title),
        body: clean_body(&base.body),
        status: base.status,
        priority: base.priority,
        labels: base
            .labels
            .iter()
            .map(|label| clean_line(label).to_lowercase())
            .collect(),
    }
}

pub fn base_of(said: &Said) -> SyncBase {
    SyncBase {
        title: said.title.clone(),
        body: said.body.clone(),
        status: said.status,
        priority: said.priority,
        labels: said.labels.iter().cloned().collect(),
    }
}

/// What to do to an issue that is already there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteChange {
    /// What to edit.
    pub edit: IssueEdit,
    /// Close it (`Some(true)`, as not planned when the second is) or open it
    /// again (`Some(false)`); `None` leaves it as it is.
    pub close: Option<(bool, bool)>,
}

/// One thing the sync does. Steps are made and carried out one after the
/// other, so the size of the larger one costs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Step {
    /// An open issue with no task: a task is made from it and linked.
    Import {
        /// The issue.
        issue: Issue,
    },
    /// A task and an issue that are linked, where one or both changed.
    Sync {
        /// The issue's number.
        number: i64,
        /// The task.
        task_id: String,
        /// When the task was last changed as the plan read it: the local
        /// write is made only if it still was.
        expected: i64,
        /// What the task takes from the issue.
        local: Option<TaskPatch>,
        /// What the issue takes from the task.
        remote: Option<RemoteChange>,
        /// The link as it is once both sides agree.
        link: TaskLink,
    },
}

/// What a plan is made from.
pub struct Input<'a> {
    /// The repository, as `owner/name`.
    pub repo: &'a str,
    /// The tasks of the project.
    pub tasks: &'a [Task],
    /// Every link of the repository, tombstones included.
    pub links: &'a [TaskLink],
    /// The issues of the repository.
    pub issues: &'a [Issue],
    /// The time now, in milliseconds.
    pub now: i64,
}

/// When an issue was last updated, in milliseconds; `None` when the text is
/// not a time.
fn millis(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|time| time.timestamp_millis())
}

/// One field of a three-way merge: the side that changed wins, and when both
/// did, the one changed last (the issue when that cannot be told).
fn pick<T: PartialEq + Clone>(base: &T, local: &T, remote: &T, local_is_newer: bool) -> T {
    if local == remote || remote == base {
        local.clone()
    } else if local == base || !local_is_newer {
        remote.clone()
    } else {
        local.clone()
    }
}

fn merge_labels(
    base: &BTreeSet<String>,
    local: &BTreeSet<String>,
    remote: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut merged = base.clone();
    for label in local.difference(base).chain(remote.difference(base)) {
        merged.insert(label.clone());
    }
    for label in base.difference(local).chain(base.difference(remote)) {
        merged.remove(label);
    }
    merged
}

/// What is to be done for the linked pair, if anything.
fn sync_pair(task: &Task, link: &TaskLink, issue: &Issue, now: i64) -> Option<Step> {
    let base = said_by_base(&link.base);
    let local = said_by_task(task);
    let remote = said_by(issue, Some(base.status));
    let local_is_newer = match millis(&issue.updated_at) {
        Some(remote_at) => task.updated_at > remote_at,
        None => false,
    };

    // A body that is a cut of the issue's is never written back, and is not
    // read again until the issue's fits.
    let remote_fits = remote.body.chars().count() <= MAX_BODY_CHARS;
    let body_follows = !link.body_cut || remote_fits;
    let title = pick(&base.title, &local.title, &remote.title, local_is_newer);
    let status = pick(&base.status, &local.status, &remote.status, local_is_newer);
    let priority = pick(
        &base.priority,
        &local.priority,
        &remote.priority,
        local_is_newer,
    );
    // The base keeps the body it had: what a task holds of it changes with
    // every edit and says nothing about the issue.
    let body = if body_follows {
        pick(&base.body, &local.body, &remote.body, local_is_newer)
    } else {
        base.body.clone()
    };
    let labels = merge_labels(&base.labels, &local.labels, &remote.labels);
    let merged = Said {
        title,
        body,
        status,
        priority,
        labels,
    };

    // What the task takes.
    let mut patch = TaskPatch::default();
    if merged.title != local.title {
        patch.title = Some(merged.title.clone());
    }
    if body_follows && merged.body != local.body {
        patch.body = Some(merged.body.clone());
    }
    if merged.status != local.status {
        patch.status = Some(merged.status);
    }
    if merged.priority != local.priority {
        patch.priority = Some(merged.priority);
    }
    if merged.labels != local.labels {
        patch.labels = Some(merged.labels.iter().cloned().collect());
    }
    // Who it is for: only an empty one is filled, and never by an agent's.
    let assignee = task
        .assignee
        .as_deref()
        .is_none_or(|name| name.trim().is_empty());
    if assignee {
        if let Some(login) = &issue.assignee {
            patch.assignee = Some(Some(login.clone()));
        }
    }

    // What the issue takes.
    let mut edit = IssueEdit::default();
    if merged.title != remote.title {
        edit.title = Some(merged.title.clone());
    }
    if body_follows && merged.body != remote.body {
        edit.body = Some(merged.body.clone());
    }
    let remote_has: BTreeSet<String> = issue
        .labels
        .iter()
        .map(|label| clean_line(label).to_lowercase())
        .collect();
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    if merged.status != remote.status {
        wanted.extend(status_label(merged.status));
    }
    if merged.priority != remote.priority {
        wanted.extend(priority_label(merged.priority));
    }
    for label in merged.labels.difference(&remote.labels) {
        wanted.insert(label.clone());
    }
    for label in &wanted {
        if !remote_has.contains(label) {
            edit.add_labels.push(label.clone());
        }
    }
    // A label that went from the task, and the markers that no longer fit.
    for label in remote.labels.difference(&merged.labels) {
        edit.remove_labels.push(label.clone());
    }
    if merged.status != remote.status {
        for label in &remote_has {
            if label.starts_with(STATUS_PREFIX)
                && Some(label) != status_label(merged.status).as_ref()
            {
                edit.remove_labels.push(label.clone());
            }
        }
    }
    if merged.priority != remote.priority {
        for label in &remote_has {
            if label.starts_with(PRIORITY_PREFIX)
                && Some(label) != priority_label(merged.priority).as_ref()
            {
                edit.remove_labels.push(label.clone());
            }
        }
    }
    edit.add_labels.sort();
    edit.remove_labels.sort();
    edit.remove_labels.dedup();
    let close = (merged.status.is_closed() != remote.status.is_closed()
        || merged.status != remote.status && merged.status.is_closed())
    .then_some((
        merged.status.is_closed(),
        merged.status == TaskStatus::Canceled,
    ));

    let local_change = (!patch.is_empty()).then_some(patch);
    let remote_change =
        (!edit.is_empty() || close.is_some()).then_some(RemoteChange { edit, close });
    let new_base = base_of(&merged);
    let body_cut = link.body_cut && !remote_fits;
    let unchanged_base = new_base == link.base && body_cut == link.body_cut;
    if local_change.is_none() && remote_change.is_none() && unchanged_base {
        return None;
    }
    Some(Step::Sync {
        number: issue.number,
        task_id: task.id.clone(),
        expected: task.updated_at,
        local: local_change,
        remote: remote_change,
        link: TaskLink {
            base: new_base,
            remote_updated_at: issue.updated_at.clone(),
            body_cut,
            synced_at: now,
            ..link.clone()
        },
    })
}

/// What the sync does, in the order of the issues' numbers.
pub fn plan(input: &Input<'_>) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut issues: Vec<&Issue> = input.issues.iter().collect();
    issues.sort_by_key(|issue| issue.number);
    for issue in issues {
        let link = input
            .links
            .iter()
            .find(|link| link.repo == input.repo && link.number == issue.number);
        match link {
            // Remembered and left alone: its task was deleted or unlinked.
            Some(link) if link.task_id.is_none() => {}
            Some(link) => {
                let task = link
                    .task_id
                    .as_deref()
                    .and_then(|id| input.tasks.iter().find(|task| task.id == id));
                // A task of another project, or one that is gone: not this
                // run's to touch.
                if let Some(task) = task {
                    steps.extend(sync_pair(task, link, issue, input.now));
                }
            }
            None if issue.open => steps.push(Step::Import {
                issue: issue.clone(),
            }),
            None => {}
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::{MemoryScope, TaskPriority};

    fn task(id: &str, title: &str, status: TaskStatus, updated_at: i64) -> Task {
        Task {
            id: id.into(),
            scope: MemoryScope::project("/srv/api"),
            title: title.into(),
            body: "Body".into(),
            status,
            priority: TaskPriority::None,
            position: 1.0,
            labels: Vec::new(),
            assignee: None,
            agent: None,
            created_at: 1,
            updated_at,
            closed_at: status.is_closed().then_some(updated_at),
            branch: None,
            parent: None,
            needs: Vec::new(),
        }
    }

    fn issue(number: i64, title: &str, labels: &[&str], open: bool) -> Issue {
        Issue {
            number,
            url: format!("https://github.com/o/r/issues/{number}"),
            title: title.into(),
            body: "Body".into(),
            open,
            not_planned: false,
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
            assignee: None,
            updated_at: "2026-10-10T10:00:00Z".into(),
        }
    }

    fn link(task_id: Option<&str>, number: i64, base: &Task, status: TaskStatus) -> TaskLink {
        TaskLink {
            provider: "github".into(),
            repo: "o/r".into(),
            number,
            url: format!("https://github.com/o/r/issues/{number}"),
            task_id: task_id.map(str::to_owned),
            base: SyncBase {
                title: base.title.clone(),
                body: base.body.clone(),
                status,
                priority: TaskPriority::None,
                labels: Vec::new(),
            },
            remote_updated_at: "2026-10-10T09:00:00Z".into(),
            body_cut: false,
            synced_at: 1,
        }
    }

    fn run(tasks: &[Task], links: &[TaskLink], issues: &[Issue]) -> Vec<Step> {
        plan(&Input {
            repo: "o/r",
            tasks,
            links,
            issues,
            now: 5_000,
        })
    }

    // 2026-10-10T10:00:00Z in milliseconds.
    const REMOTE_AT: i64 = 1791626400000;

    #[test]
    fn a_task_and_an_issue_that_agree_make_no_step_whatever_the_spelling() {
        let mut t = task("a", "Fix the login", TaskStatus::Doing, 10);
        t.labels = vec!["Bug".into()];
        t.priority = TaskPriority::High;
        let mut l = link(Some("a"), 1, &t, TaskStatus::Doing);
        l.base.priority = TaskPriority::High;
        l.base.labels = vec!["bug".into()];
        // The issue spells it its own way: CRLF, case, a marker for each.
        let mut i = issue(
            1,
            "Fix  the login",
            &["BUG", "status:doing", "priority:high"],
            true,
        );
        i.body = "Body\r\n".into();
        assert_eq!(run(&[t], &[l], &[i]), []);
    }

    #[test]
    fn a_change_on_the_issue_alone_is_pulled_and_on_the_task_alone_is_pushed() {
        let t = task("a", "Fix the login", TaskStatus::Todo, 10);
        let l = link(Some("a"), 1, &t, TaskStatus::Todo);
        // The issue was renamed and moved to doing there.
        let pulled = run(
            std::slice::from_ref(&t),
            std::slice::from_ref(&l),
            &[issue(1, "Fix the SSO login", &["status:doing"], true)],
        );
        let [Step::Sync {
            local: Some(local),
            remote: None,
            link: after,
            ..
        }] = pulled.as_slice()
        else {
            panic!("{pulled:?}");
        };
        assert_eq!(local.title.as_deref(), Some("Fix the SSO login"));
        assert_eq!(local.status, Some(TaskStatus::Doing));
        assert_eq!(after.base.title, "Fix the SSO login");
        assert_eq!(after.base.status, TaskStatus::Doing);

        // The task was moved to review here: the issue gets the new marker
        // and loses the old one.
        let mut moved = t.clone();
        moved.status = TaskStatus::Review;
        moved.updated_at = 20;
        let mut doing = l.clone();
        doing.base.status = TaskStatus::Doing;
        let pushed = run(
            &[moved],
            &[doing],
            &[issue(1, "Fix the login", &["status:doing"], true)],
        );
        let [Step::Sync {
            local: None,
            remote: Some(remote),
            ..
        }] = pushed.as_slice()
        else {
            panic!("{pushed:?}");
        };
        assert_eq!(remote.edit.add_labels, ["status:review"]);
        assert_eq!(remote.edit.remove_labels, ["status:doing"]);
        assert_eq!(remote.close, None);
    }

    #[test]
    fn a_field_changed_on_both_sides_is_decided_by_the_later_change() {
        let mut t = task("a", "Local title", TaskStatus::Todo, REMOTE_AT + 1);
        let l = link(
            Some("a"),
            1,
            &task("a", "Base title", TaskStatus::Todo, 1),
            TaskStatus::Todo,
        );
        let i = issue(1, "Remote title", &["status:todo"], true);
        // The task was changed after the issue: its title wins.
        let steps = run(
            std::slice::from_ref(&t),
            std::slice::from_ref(&l),
            std::slice::from_ref(&i),
        );
        let [Step::Sync {
            remote: Some(r),
            local: None,
            ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        assert_eq!(r.edit.title.as_deref(), Some("Local title"));
        // The issue was changed after the task: its title wins.
        t.updated_at = REMOTE_AT - 1;
        let steps = run(&[t], &[l], &[i]);
        let [Step::Sync {
            local: Some(p),
            remote: None,
            ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        assert_eq!(p.title.as_deref(), Some("Remote title"));
    }

    #[test]
    fn labels_are_merged_as_sets_so_an_addition_on_either_side_survives() {
        let mut t = task("a", "Fix", TaskStatus::Todo, 10);
        t.labels = vec!["mine".into(), "shared".into()];
        let mut l = link(Some("a"), 1, &t, TaskStatus::Todo);
        l.base.labels = vec!["shared".into(), "dropped-here".into()];
        // The task added `mine` and removed `dropped-here`; the issue added
        // `theirs`.
        let i = issue(
            1,
            "Fix",
            &["shared", "theirs", "dropped-here", "status:todo"],
            true,
        );
        let steps = run(&[t], &[l], &[i]);
        let [Step::Sync {
            local: Some(p),
            remote: Some(r),
            link,
            ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        let mut local: Vec<_> = p.labels.clone().unwrap();
        local.sort();
        assert_eq!(local, ["mine", "shared", "theirs"]);
        assert_eq!(r.edit.add_labels, ["mine"]);
        assert_eq!(r.edit.remove_labels, ["dropped-here"]);
        assert_eq!(link.base.labels, ["mine", "shared", "theirs"]);
    }

    #[test]
    fn done_and_canceled_close_the_issue_and_a_task_that_comes_back_opens_it() {
        let base = task("a", "Fix", TaskStatus::Doing, 1);
        let l = link(Some("a"), 1, &base, TaskStatus::Doing);
        let open = issue(1, "Fix", &["status:doing"], true);
        let mut done = base.clone();
        done.status = TaskStatus::Done;
        done.updated_at = 20;
        let steps = run(
            &[done],
            std::slice::from_ref(&l),
            std::slice::from_ref(&open),
        );
        let [Step::Sync {
            remote: Some(r), ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        assert_eq!(r.close, Some((true, false)));
        let mut canceled = base.clone();
        canceled.status = TaskStatus::Canceled;
        canceled.updated_at = 20;
        let steps = run(&[canceled], &[l], &[open]);
        let [Step::Sync {
            remote: Some(r), ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        assert_eq!(r.close, Some((true, true)));

        // The issue was closed there as not planned: the task is canceled.
        let mut closed = issue(1, "Fix", &[], false);
        closed.not_planned = true;
        let t = task("a", "Fix", TaskStatus::Doing, 5);
        let l = link(Some("a"), 1, &t, TaskStatus::Doing);
        let steps = run(&[t], &[l], &[closed]);
        let [Step::Sync {
            local: Some(p),
            remote: None,
            ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        assert_eq!(p.status, Some(TaskStatus::Canceled));

        // The task was reopened here: the issue is opened again with its
        // marker.
        let closed_base = task("a", "Fix", TaskStatus::Done, 1);
        let l = link(Some("a"), 1, &closed_base, TaskStatus::Done);
        let mut reopened = closed_base.clone();
        reopened.status = TaskStatus::Doing;
        reopened.updated_at = 30;
        let steps = run(&[reopened], &[l], &[issue(1, "Fix", &[], false)]);
        let [Step::Sync {
            remote: Some(r), ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        assert_eq!(r.close, Some((false, false)));
        assert_eq!(r.edit.add_labels, ["status:doing"]);
    }

    #[test]
    fn only_open_issues_without_a_link_are_imported_and_a_tombstone_keeps_one_out() {
        let t = task("a", "Mine", TaskStatus::Todo, 1);
        let tomb = link(None, 3, &t, TaskStatus::Todo);
        let issues = [
            issue(1, "New and open", &["status:review"], true),
            issue(2, "Old and closed", &[], false),
            issue(3, "Deleted here", &[], true),
        ];
        let steps = run(&[t], &[tomb], &issues);
        assert_eq!(steps.len(), 1, "{steps:?}");
        let Step::Import { issue } = &steps[0] else {
            panic!("{steps:?}");
        };
        assert_eq!(issue.number, 1);
        // What it becomes is read by `said_by`.
        assert_eq!(said_by(issue, None).status, TaskStatus::Review);
    }

    #[test]
    fn an_open_issue_without_a_marker_is_todo_unless_the_task_was_open_already() {
        let bare = issue(1, "Fix", &[], true);
        assert_eq!(said_by(&bare, None).status, TaskStatus::Todo);
        assert_eq!(
            said_by(&bare, Some(TaskStatus::Done)).status,
            TaskStatus::Todo
        );
        assert_eq!(
            said_by(&bare, Some(TaskStatus::Doing)).status,
            TaskStatus::Doing
        );
        let wears = issue(1, "Fix", &["Status:Review", "Priority:URGENT"], true);
        let said = said_by(&wears, Some(TaskStatus::Doing));
        assert_eq!(
            (said.status, said.priority),
            (TaskStatus::Review, TaskPriority::Urgent)
        );
        assert!(said.labels.is_empty(), "the markers are not labels");
    }

    #[test]
    fn a_body_cut_to_fit_a_task_is_never_written_back() {
        let mut t = task("a", "Fix", TaskStatus::Todo, 10);
        t.body = "x".repeat(MAX_BODY_CHARS);
        let mut l = link(Some("a"), 1, &t, TaskStatus::Todo);
        l.base.body = "x".repeat(MAX_BODY_CHARS);
        l.body_cut = true;
        let mut i = issue(1, "Fix", &["status:todo"], true);
        i.body = "x".repeat(MAX_BODY_CHARS + 500);
        // Neither side's body is touched, and the pair is at rest.
        assert_eq!(
            run(
                std::slice::from_ref(&t),
                std::slice::from_ref(&l),
                std::slice::from_ref(&i)
            ),
            []
        );
        // The task's body was edited: still not pushed.
        let mut edited = t.clone();
        edited.body = "y".repeat(100);
        edited.updated_at = REMOTE_AT + 5;
        assert_eq!(
            run(
                &[edited],
                std::slice::from_ref(&l),
                std::slice::from_ref(&i)
            ),
            []
        );
        // The issue's body shrank to what a task holds: it is read again and
        // the link says the body is whole.
        i.body = "short".into();
        let steps = run(&[t], &[l], &[i]);
        let [Step::Sync {
            local: Some(p),
            remote: None,
            link,
            ..
        }] = steps.as_slice()
        else {
            panic!("{steps:?}");
        };
        assert_eq!(p.body.as_deref(), Some("short"));
        assert!(!link.body_cut);
    }

    #[test]
    fn an_empty_assignee_is_filled_from_the_issue_and_never_overwritten() {
        let t = task("a", "Fix", TaskStatus::Todo, 10);
        let l = link(Some("a"), 1, &t, TaskStatus::Todo);
        let mut i = issue(1, "Fix", &["status:todo"], true);
        i.assignee = Some("ana".into());
        let steps = run(
            std::slice::from_ref(&t),
            std::slice::from_ref(&l),
            std::slice::from_ref(&i),
        );
        let [Step::Sync { local: Some(p), .. }] = steps.as_slice() else {
            panic!("{steps:?}");
        };
        assert_eq!(p.assignee, Some(Some("ana".to_owned())));
        // An agent has it: the issue's assignee does not take it.
        let mut agent = t;
        agent.assignee = Some("claude".into());
        assert_eq!(run(&[agent], &[l], &[i]), []);
    }

    #[test]
    fn what_a_task_cannot_hold_is_not_compared_so_it_is_never_pushed_away() {
        // An issue with a 300 character title and fifteen labels, imported:
        // the task holds a cut title and ten labels.
        let long = "w".repeat(300);
        let names: Vec<String> = (0..15).map(|n| format!("label-{n:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let big = issue(1, &long, &refs, true);
        let said = said_by(&big, None);
        assert_eq!(said.title.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(said.labels.len(), MAX_LABELS);
        // The task made from it is the same, so the pair is at rest.
        let mut t = task("a", &said.title, TaskStatus::Todo, 10);
        t.labels = said.labels.iter().cloned().collect();
        let mut l = link(Some("a"), 1, &t, TaskStatus::Todo);
        l.base = base_of(&said);
        assert_eq!(run(&[t], &[l], &[big]), []);
    }

    #[test]
    fn a_link_of_a_task_that_is_not_in_the_project_is_not_touched() {
        let t = task("a", "Fix", TaskStatus::Todo, 10);
        let l = link(Some("elsewhere"), 1, &t, TaskStatus::Todo);
        assert_eq!(run(&[t], &[l], &[issue(1, "Changed there", &[], true)]), []);
    }

    #[test]
    fn the_markers_a_repository_needs_are_the_ones_the_mapping_writes() {
        let names: Vec<String> = marker_labels()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(
            names,
            [
                "status:backlog",
                "status:todo",
                "status:doing",
                "status:review",
                "priority:low",
                "priority:medium",
                "priority:high",
                "priority:urgent",
            ]
        );
    }
}
