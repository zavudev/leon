//! The board: the tasks a person and their agents keep for a project.
//!
//! A task is a title, a body and a place on the board: a [`TaskStatus`] (the
//! column) and a position inside it. It belongs to the same scope a memory
//! entry does ([`MemoryScope`]): one project, identified by the identity of
//! its root folder so that every git worktree of it shares one board, or
//! everything. Agents write here through `leon task` and the MCP server, the
//! person through the board; both end in the calls below.
//!
//! What keeps the board from filling with noise:
//!
//! * **The same open title is one task.** Creating a task whose title (case,
//!   spacing and surrounding punctuation aside) an open task of the scope
//!   already has creates nothing and answers with that one
//!   ([`SavedTask::Known`]). An agent that retries, or two agents that find
//!   the same chore, do not duplicate it. A closed task does not count: the
//!   same chore can come back.
//! * **Everything is bounded**: title, body, labels, assignee, and the tasks
//!   of a scope ([`MAX_TASKS`], a guard against an agent that creates in a
//!   loop). What is refused is refused with a [`TaskError`].
//!
//! **A task can be a step of another.** It names its parent, and the tasks
//! of the same parent it needs done before it can start (its *needs*): the
//! steps of a run King carries out in that order. Neither crosses a board,
//! a task is never its own ancestor, and needs never go round in a circle.
//!
//! Cards inside a column are ordered by a real-number position, so a card goes
//! between two others by writing one row. The clock is the caller's: every
//! write takes its time in milliseconds.

use rusqlite::{params, OptionalExtension as _, Row, Transaction};
use thiserror::Error;

use super::history::sql_limit;
use super::memory::{
    agent_tag, clean_text, one_line, text_hash, MemoryScope, GLOBAL_SCOPE, MIN_ID_TAIL,
};
use super::search::{fts_query, SNIPPET_ELLIPSIS, SNIPPET_END, SNIPPET_START};
use super::Store;
use crate::change::StoreChange;
use crate::error::{Result, StoreError};

/// The longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// The longest body, in characters.
pub const MAX_BODY_CHARS: usize = 20_000;
/// The most labels a task has.
pub const MAX_LABELS: usize = 10;
/// The longest label, in characters.
pub const MAX_LABEL_CHARS: usize = 40;
/// The longest assignee, in characters.
pub const MAX_ASSIGNEE_CHARS: usize = 64;
/// The longest branch name, in characters.
pub const MAX_BRANCH_CHARS: usize = 200;
/// The most tasks one scope holds.
pub const MAX_TASKS: usize = 100_000;
/// The most tasks or hits one call returns, whatever it asks for.
pub const MAX_LISTED: usize = 500;

/// Roughly how many words a snippet contains.
const SNIPPET_WORDS: u32 = 24;

/// The columns of a task, in the order [`task_from_row`] reads them.
const COLUMNS: &str = "t.id, t.scope, t.root, t.title, t.body, t.status, t.priority, \
                       t.position, t.labels, t.assignee, t.agent, t.created_at, \
                       t.updated_at, t.closed_at, t.branch, t.parent, t.needs";
/// How many columns that is: where a query's own columns start.
const COLUMN_COUNT: usize = 17;

/// Why a task was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskError {
    /// There is no title.
    #[error("a task needs a title")]
    EmptyTitle,
    /// The title is longer than [`MAX_TITLE_CHARS`].
    #[error("the title has {found} characters and the most is {max}")]
    TitleTooLong {
        /// How long it is.
        found: usize,
        /// The most allowed.
        max: usize,
    },
    /// The body is longer than [`MAX_BODY_CHARS`].
    #[error("the body has {found} characters and the most is {max}")]
    BodyTooLong {
        /// How long it is.
        found: usize,
        /// The most allowed.
        max: usize,
    },
    /// A label is empty or longer than [`MAX_LABEL_CHARS`].
    #[error("{0:?} is not a label: one to {MAX_LABEL_CHARS} characters")]
    BadLabel(String),
    /// More than [`MAX_LABELS`] labels.
    #[error("a task has at most {MAX_LABELS} labels")]
    TooManyLabels,
    /// The assignee is longer than [`MAX_ASSIGNEE_CHARS`].
    #[error("{0:?} is not an assignee: at most {MAX_ASSIGNEE_CHARS} characters")]
    BadAssignee(String),
    /// The word is not one of [`TaskStatus::ALL`].
    #[error("{0:?} is not a status: use backlog, todo, doing, review, done or canceled")]
    UnknownStatus(String),
    /// The word is not one of [`TaskPriority::ALL`].
    #[error("{0:?} is not a priority: use none, low, medium, high or urgent")]
    UnknownPriority(String),
    /// The scope already holds [`MAX_TASKS`].
    #[error("this board is full ({max} tasks): delete some before adding more")]
    BoardFull {
        /// The most a scope holds.
        max: usize,
    },
    /// A tail of an id names more than one task.
    #[error("{0:?} is the end of more than one id: give more of it")]
    AmbiguousId(String),
    /// The branch is empty, too long or not one line.
    #[error("{0:?} is not a branch: one line of at most {MAX_BRANCH_CHARS} characters")]
    BadBranch(String),
    /// A change that changes nothing.
    #[error(
        "nothing to change: give a title, body, status, priority, labels, assignee, branch, \
         parent or needs"
    )]
    NothingToChange,
    /// The parent is the task itself, one of its steps, or on another board.
    #[error("{0:?} cannot be its parent: a task of the same board, and not one of its steps")]
    BadParent(String),
    /// A need is the task itself, on another board, or makes a circle.
    #[error(
        "{0:?} cannot be needed: a task of the same board that does not, itself, wait for this one"
    )]
    BadNeed(String),
}

impl From<TaskError> for StoreError {
    fn from(error: TaskError) -> Self {
        StoreError::Invalid(error.to_string())
    }
}

/// A column of the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TaskStatus {
    /// Known, not planned.
    Backlog,
    /// Planned, not started.
    Todo,
    /// Being worked on.
    Doing,
    /// Done, waiting to be looked at.
    Review,
    /// Finished.
    Done,
    /// Dropped without being done.
    Canceled,
}

impl TaskStatus {
    /// Every status, in the order of the board.
    pub const ALL: [TaskStatus; 6] = [
        TaskStatus::Backlog,
        TaskStatus::Todo,
        TaskStatus::Doing,
        TaskStatus::Review,
        TaskStatus::Done,
        TaskStatus::Canceled,
    ];

    /// The status as it is stored and typed.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Backlog => "backlog",
            TaskStatus::Todo => "todo",
            TaskStatus::Doing => "doing",
            TaskStatus::Review => "review",
            TaskStatus::Done => "done",
            TaskStatus::Canceled => "canceled",
        }
    }

    /// Reads a status as it is typed: the names above, and the spellings the
    /// trackers use for the same columns.
    pub fn parse(word: &str) -> Result<Self, TaskError> {
        match word.trim().to_lowercase().replace([' ', '_'], "-").as_str() {
            "backlog" => Ok(TaskStatus::Backlog),
            "todo" | "to-do" | "open" | "unstarted" => Ok(TaskStatus::Todo),
            "doing" | "in-progress" | "started" | "wip" => Ok(TaskStatus::Doing),
            "review" | "in-review" => Ok(TaskStatus::Review),
            "done" | "closed" | "completed" => Ok(TaskStatus::Done),
            "canceled" | "cancelled" | "wontfix" => Ok(TaskStatus::Canceled),
            _ => Err(TaskError::UnknownStatus(word.to_owned())),
        }
    }

    /// Whether a task in this status is finished one way or another.
    pub fn is_closed(self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Canceled)
    }
}

/// How much a task matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TaskPriority {
    /// Not set.
    None,
    /// It can wait.
    Low,
    /// The usual.
    Medium,
    /// Soon.
    High,
    /// Now.
    Urgent,
}

impl TaskPriority {
    /// Every priority, lowest first.
    pub const ALL: [TaskPriority; 5] = [
        TaskPriority::None,
        TaskPriority::Low,
        TaskPriority::Medium,
        TaskPriority::High,
        TaskPriority::Urgent,
    ];

    /// The priority as it is stored and typed.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskPriority::None => "none",
            TaskPriority::Low => "low",
            TaskPriority::Medium => "medium",
            TaskPriority::High => "high",
            TaskPriority::Urgent => "urgent",
        }
    }

    /// The number the store keeps: 0 (none) to 4 (urgent), the scale the
    /// trackers use.
    pub fn level(self) -> i64 {
        self as i64
    }

    /// The priority of a stored number; anything out of range is none.
    pub fn from_level(level: i64) -> Self {
        usize::try_from(level)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
            .unwrap_or(TaskPriority::None)
    }

    /// Reads a priority as it is typed: a name, or 0 to 4.
    pub fn parse(word: &str) -> Result<Self, TaskError> {
        let word = word.trim().to_lowercase();
        if let Ok(level) = word.parse::<i64>() {
            if (0..=4).contains(&level) {
                return Ok(Self::from_level(level));
            }
        }
        match word.as_str() {
            "none" | "no" | "" => Ok(TaskPriority::None),
            "low" => Ok(TaskPriority::Low),
            "medium" | "normal" | "med" => Ok(TaskPriority::Medium),
            "high" => Ok(TaskPriority::High),
            "urgent" | "critical" => Ok(TaskPriority::Urgent),
            _ => Err(TaskError::UnknownPriority(word)),
        }
    }
}

/// A task to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    /// Whose board.
    pub scope: MemoryScope,
    /// What is to be done.
    pub title: String,
    /// The detail; may be empty.
    pub body: String,
    /// The column it starts in; [`TaskStatus::Todo`] when absent.
    pub status: Option<TaskStatus>,
    /// How much it matters; none when absent.
    pub priority: Option<TaskPriority>,
    /// Its labels.
    pub labels: Vec<String>,
    /// Who is to do it: a person or an agent.
    pub assignee: Option<String>,
    /// Who created it, when known.
    pub agent: Option<String>,
}

impl NewTask {
    /// A task with only a title, in the default column.
    pub fn titled(scope: MemoryScope, title: impl Into<String>) -> Self {
        Self {
            scope,
            title: title.into(),
            body: String::new(),
            status: None,
            priority: None,
            labels: Vec::new(),
            assignee: None,
            agent: None,
        }
    }
}

/// A change to a task. What is `None` stays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskPatch {
    /// A new title.
    pub title: Option<String>,
    /// A new body.
    pub body: Option<String>,
    /// A new column (the task goes to the bottom of it).
    pub status: Option<TaskStatus>,
    /// A new priority.
    pub priority: Option<TaskPriority>,
    /// New labels, replacing the old ones.
    pub labels: Option<Vec<String>>,
    /// A new assignee, or none any more (`Some(None)`).
    pub assignee: Option<Option<String>>,
    /// The branch of the worktree made for the task, or none any more.
    pub branch: Option<Option<String>>,
    /// The task it is a step of, or none any more.
    pub parent: Option<Option<String>>,
    /// The tasks it waits for, replacing the old ones.
    pub needs: Option<Vec<String>>,
}

impl TaskPatch {
    /// Whether the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Which tasks a listing keeps. The default keeps every task of the scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFilter {
    /// Only this column.
    pub status: Option<TaskStatus>,
    /// Leave out the done and canceled ones.
    pub open_only: bool,
    /// Only the tasks of this assignee (case does not count).
    pub assignee: Option<String>,
    /// Only the tasks with this label.
    pub label: Option<String>,
    /// Only the steps of this task, by its whole id.
    pub parent: Option<String>,
}

/// A task as it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// The public identifier.
    pub id: String,
    /// Whose board.
    pub scope: MemoryScope,
    /// What is to be done.
    pub title: String,
    /// The detail.
    pub body: String,
    /// The column.
    pub status: TaskStatus,
    /// How much it matters.
    pub priority: TaskPriority,
    /// Its place inside the column: the lower, the higher up.
    pub position: f64,
    /// Its labels, lower case.
    pub labels: Vec<String>,
    /// Who is to do it.
    pub assignee: Option<String>,
    /// Who created it, when known.
    pub agent: Option<String>,
    /// When it was created, in milliseconds.
    pub created_at: i64,
    /// When it last changed, in milliseconds.
    pub updated_at: i64,
    /// When it was closed, while it is done or canceled.
    pub closed_at: Option<i64>,
    /// The branch of the worktree made for it, when work was started.
    pub branch: Option<String>,
    /// The task it is a step of, by its whole id.
    pub parent: Option<String>,
    /// The tasks it waits for, by their whole ids.
    pub needs: Vec<String>,
}

impl Task {
    /// The eight characters shown for the task and enough to name it.
    pub fn short_id(&self) -> &str {
        super::memory::short_id(&self.id)
    }
}

/// What creating a task did.
#[derive(Debug, Clone, PartialEq)]
pub enum SavedTask {
    /// A new task.
    New(Task),
    /// An open task of the scope already had this title; nothing was added.
    Known(Task),
}

impl SavedTask {
    /// The task, new or known.
    pub fn task(&self) -> &Task {
        match self {
            SavedTask::New(task) | SavedTask::Known(task) => task,
        }
    }

    /// The task, new or known, by value.
    pub fn into_task(self) -> Task {
        match self {
            SavedTask::New(task) | SavedTask::Known(task) => task,
        }
    }
}

/// A task a search found, with the words around the match.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskHit {
    /// The task.
    pub task: Task,
    /// The excerpt of the body, the match between the snippet markers.
    pub snippet: String,
    /// The index's rank: lower is better.
    pub rank: f64,
}

fn checked_title(title: &str) -> Result<String, TaskError> {
    let title = one_line(title);
    let found = title.chars().count();
    if found == 0 {
        return Err(TaskError::EmptyTitle);
    }
    if found > MAX_TITLE_CHARS {
        return Err(TaskError::TitleTooLong {
            found,
            max: MAX_TITLE_CHARS,
        });
    }
    Ok(title)
}

/// A line of text as the store keeps it, never refused.
fn clean_text_line(text: &str) -> String {
    one_line(text)
}

/// A body as the store keeps it, never refused for its length.
fn checked_body_text(text: &str) -> String {
    clean_text(text)
}

fn checked_body(body: &str) -> Result<String, TaskError> {
    let body = clean_text(body);
    let found = body.chars().count();
    if found > MAX_BODY_CHARS {
        return Err(TaskError::BodyTooLong {
            found,
            max: MAX_BODY_CHARS,
        });
    }
    Ok(body)
}

/// One label as it is kept: lower case, one line.
fn checked_label(label: &str) -> Result<String, TaskError> {
    let one = one_line(label).to_lowercase();
    if one.is_empty() || one.chars().count() > MAX_LABEL_CHARS {
        return Err(TaskError::BadLabel(label.to_owned()));
    }
    Ok(one)
}

/// The labels as they are kept: each checked, repeats dropped, in order.
pub fn checked_labels(labels: &[String]) -> Result<Vec<String>, TaskError> {
    let mut kept: Vec<String> = Vec::new();
    for label in labels {
        let label = checked_label(label)?;
        if !kept.contains(&label) {
            kept.push(label);
        }
    }
    if kept.len() > MAX_LABELS {
        return Err(TaskError::TooManyLabels);
    }
    Ok(kept)
}

fn checked_assignee(assignee: Option<&str>) -> Result<Option<String>, TaskError> {
    let Some(given) = assignee else {
        return Ok(None);
    };
    let one = one_line(given);
    if one.is_empty() {
        return Ok(None);
    }
    if one.chars().count() > MAX_ASSIGNEE_CHARS {
        return Err(TaskError::BadAssignee(given.to_owned()));
    }
    Ok(Some(one))
}

fn checked_branch(branch: Option<&str>) -> Result<Option<String>, TaskError> {
    let Some(given) = branch else {
        return Ok(None);
    };
    let branch = given.trim();
    if branch.is_empty() {
        return Ok(None);
    }
    if branch.chars().count() > MAX_BRANCH_CHARS
        || branch.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(TaskError::BadBranch(given.to_owned()));
    }
    Ok(Some(branch.to_owned()))
}

/// Lines kept one per value, as labels and needs are.
fn split_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
    let scope: String = row.get(1)?;
    let root: String = row.get(2)?;
    let status: String = row.get(5)?;
    let labels: String = row.get(8)?;
    Ok(Task {
        id: row.get(0)?,
        scope: if scope == GLOBAL_SCOPE {
            MemoryScope::Global
        } else {
            MemoryScope::Project(root)
        },
        title: row.get(3)?,
        body: row.get(4)?,
        status: TaskStatus::parse(&status).map_err(|_| super::bad_tag(5, &status))?,
        priority: TaskPriority::from_level(row.get(6)?),
        position: row.get(7)?,
        labels: split_lines(&labels),
        assignee: row.get(9)?,
        agent: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        closed_at: row.get(13)?,
        branch: row.get(14)?,
        parent: row.get(15)?,
        needs: split_lines(&row.get::<_, String>(16)?),
    })
}

fn row_of(connection: &rusqlite::Connection, id: &str) -> Result<Task> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM task t WHERE t.id = ?1"),
            [id],
            task_from_row,
        )
        .optional()?
        .ok_or(StoreError::NotFound("task"))
}

/// The position below every card of the column (the first, 1, in an empty
/// one), leaving `except` out: the card being moved.
fn bottom(
    tx: &Transaction<'_>,
    scope: &str,
    status: TaskStatus,
    except: Option<&str>,
) -> Result<f64> {
    let last: Option<f64> = tx.query_row(
        "SELECT max(position) FROM task
         WHERE scope = ?1 AND status = ?2 AND (?3 IS NULL OR id <> ?3)",
        params![scope, status.as_str(), except],
        |row| row.get(0),
    )?;
    Ok(last.map_or(1.0, |last| last.floor() + 1.0))
}

/// The position that puts a card right above `before` in the column; the
/// bottom when `before` is not a card of the column.
fn above(
    tx: &Transaction<'_>,
    scope: &str,
    status: TaskStatus,
    before: &str,
    except: &str,
) -> Result<f64> {
    let target: Option<f64> = tx
        .query_row(
            "SELECT position FROM task WHERE scope = ?1 AND status = ?2 AND id = ?3",
            params![scope, status.as_str(), before],
            |row| row.get(0),
        )
        .optional()?;
    let Some(target) = target else {
        return bottom(tx, scope, status, Some(except));
    };
    let previous: Option<f64> = tx.query_row(
        "SELECT max(position) FROM task
         WHERE scope = ?1 AND status = ?2 AND position < ?3 AND id <> ?4",
        params![scope, status.as_str(), target, except],
        |row| row.get(0),
    )?;
    Ok(previous.map_or(target - 1.0, |previous| (previous + target) / 2.0))
}

/// `CASE` that orders rows by the board's column order.
fn status_order() -> String {
    let arms: String = TaskStatus::ALL
        .iter()
        .enumerate()
        .map(|(index, status)| format!(" WHEN '{}' THEN {index}", status.as_str()))
        .collect();
    format!("CASE t.status{arms} ELSE 99 END")
}

impl Store {
    /// Creates a task at `at` (milliseconds since the Unix epoch), at the
    /// bottom of its column. An open task of the scope with the same title
    /// is not duplicated: it is answered ([`SavedTask::Known`]).
    pub fn add_task(&self, new: &NewTask, at: i64) -> Result<SavedTask> {
        self.add_task_with(new, at, true)
    }

    /// Makes a task from an issue of a tracker: what [`Store::add_task`]
    /// makes, but never answers another task for it (two issues of one title
    /// are two tasks), and cuts what is too long for a task instead of
    /// refusing it. `Ok` carries whether the body was cut: a cut body must
    /// never be written back to the issue.
    pub fn import_task(&self, new: &NewTask, at: i64) -> Result<(Task, bool)> {
        let mut clamped = new.clone();
        let title: String = clean_text_line(&new.title)
            .chars()
            .take(MAX_TITLE_CHARS)
            .collect();
        clamped.title = if title.trim().is_empty() {
            "(untitled)".to_owned()
        } else {
            title
        };
        let body = checked_body_text(&new.body);
        let cut = body.chars().count() > MAX_BODY_CHARS;
        clamped.body = body.chars().take(MAX_BODY_CHARS).collect();
        let mut labels: Vec<String> = Vec::new();
        for label in &new.labels {
            let label: String = clean_text_line(label)
                .to_lowercase()
                .chars()
                .take(MAX_LABEL_CHARS)
                .collect();
            if !label.is_empty() && !labels.contains(&label) && labels.len() < MAX_LABELS {
                labels.push(label);
            }
        }
        clamped.labels = labels;
        clamped.assignee = new
            .assignee
            .as_deref()
            .map(|name| name.chars().take(MAX_ASSIGNEE_CHARS).collect::<String>());
        match self.add_task_with(&clamped, at, false)? {
            SavedTask::New(task) | SavedTask::Known(task) => Ok((task, cut)),
        }
    }

    fn add_task_with(&self, new: &NewTask, at: i64, dedup: bool) -> Result<SavedTask> {
        new.scope.check()?;
        let title = checked_title(&new.title)?;
        let body = checked_body(&new.body)?;
        let labels = checked_labels(&new.labels)?;
        let assignee = checked_assignee(new.assignee.as_deref())?;
        let agent = new.agent.as_deref().map(agent_tag).transpose()?;
        let status = new.status.unwrap_or(TaskStatus::Todo);
        let priority = new.priority.unwrap_or(TaskPriority::None);
        let key = new.scope.key();
        let hash = text_hash(&title);
        self.write(StoreChange::Tasks, |tx| {
            let known = if dedup {
                tx.query_row(
                    &format!(
                        "SELECT {COLUMNS} FROM task t
                         WHERE t.scope = ?1 AND t.title_hash = ?2 AND t.closed_at IS NULL
                         ORDER BY t.pk LIMIT 1"
                    ),
                    params![key, hash],
                    task_from_row,
                )
                .optional()?
            } else {
                None
            };
            if let Some(known) = known {
                return Ok(SavedTask::Known(known));
            }
            let held: i64 = tx.query_row(
                "SELECT count(*) FROM task WHERE scope = ?1",
                [&key],
                |row| row.get(0),
            )?;
            if usize::try_from(held).unwrap_or(usize::MAX) >= MAX_TASKS {
                return Err(TaskError::BoardFull { max: MAX_TASKS }.into());
            }
            let position = bottom(tx, &key, status, None)?;
            let task = Task {
                id: uuid::Uuid::now_v7().to_string(),
                scope: new.scope.clone(),
                title,
                body,
                status,
                priority,
                position,
                labels,
                assignee,
                agent,
                created_at: at,
                updated_at: at,
                closed_at: status.is_closed().then_some(at),
                branch: None,
                parent: None,
                needs: Vec::new(),
            };
            tx.execute(
                "INSERT INTO task (id, scope, root, title, body, status, priority, position,
                                   labels, assignee, agent, title_hash, created_at, updated_at,
                                   closed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
                params![
                    task.id,
                    key,
                    task.scope.root().unwrap_or_default(),
                    task.title,
                    task.body,
                    task.status.as_str(),
                    task.priority.level(),
                    task.position,
                    task.labels.join("\n"),
                    task.assignee,
                    task.agent,
                    hash,
                    at,
                    task.closed_at
                ],
            )?;
            Ok(SavedTask::New(task))
        })
    }

    /// Changes a task at `at` and returns it as it now is. A new status puts
    /// it at the bottom of that column and sets or clears its closing time.
    pub fn update_task(&self, id: &str, patch: &TaskPatch, at: i64) -> Result<Task> {
        match self.update_task_if(id, None, patch, at)? {
            Some(task) => Ok(task),
            None => Err(StoreError::NotFound("task")),
        }
    }

    /// [`Store::update_task`] that writes only when the task was last changed
    /// at `expected` (when that is given): `Ok(None)` when somebody changed it
    /// since, so a sync that read the task a moment ago does not overwrite an
    /// edit made in between.
    pub fn update_task_if(
        &self,
        id: &str,
        expected: Option<i64>,
        patch: &TaskPatch,
        at: i64,
    ) -> Result<Option<Task>> {
        if patch.is_empty() {
            return Err(TaskError::NothingToChange.into());
        }
        let title = patch.title.as_deref().map(checked_title).transpose()?;
        let body = patch.body.as_deref().map(checked_body).transpose()?;
        let labels = patch.labels.as_deref().map(checked_labels).transpose()?;
        let assignee = match &patch.assignee {
            Some(given) => Some(checked_assignee(given.as_deref())?),
            None => None,
        };
        let branch = match &patch.branch {
            Some(given) => Some(checked_branch(given.as_deref())?),
            None => None,
        };
        let id = self.task_id(id)?;
        let parent = match &patch.parent {
            Some(Some(given)) => {
                Some(Some(self.task_id(given).map_err(|_| {
                    StoreError::from(TaskError::BadParent(given.clone()))
                })?))
            }
            Some(None) => Some(None),
            None => None,
        };
        let needs = match &patch.needs {
            Some(given) => {
                let mut ids: Vec<String> = Vec::new();
                for need in given.iter().filter(|need| !need.trim().is_empty()) {
                    let need = self
                        .task_id(need)
                        .map_err(|_| StoreError::from(TaskError::BadNeed(need.clone())))?;
                    if !ids.contains(&need) {
                        ids.push(need);
                    }
                }
                Some(ids)
            }
            None => None,
        };
        self.write(StoreChange::Tasks, |tx| {
            let mut task = row_of(tx, &id)?;
            if expected.is_some_and(|expected| task.updated_at != expected) {
                return Ok(None);
            }
            let key = task.scope.key();
            if let Some(parent) = parent {
                if let Some(parent) = &parent {
                    check_parent(tx, &task, parent)?;
                }
                task.parent = parent;
            }
            if let Some(needs) = needs {
                for need in &needs {
                    check_need(tx, &task, need)?;
                }
                task.needs = needs;
            }
            if let Some(branch) = branch {
                task.branch = branch;
            }
            if let Some(title) = title {
                task.title = title;
            }
            if let Some(body) = body {
                task.body = body;
            }
            if let Some(priority) = patch.priority {
                task.priority = priority;
            }
            if let Some(labels) = labels {
                task.labels = labels;
            }
            if let Some(assignee) = assignee {
                task.assignee = assignee;
            }
            if let Some(status) = patch.status.filter(|status| *status != task.status) {
                task.status = status;
                task.position = bottom(tx, &key, status, Some(&id))?;
                task.closed_at = status.is_closed().then_some(at);
            }
            task.updated_at = at;
            write_task(tx, &task)?;
            Ok(Some(task))
        })
    }

    /// Moves a task to `status` at `at`: right above the task `before` when
    /// that is a card of the column, else to the bottom of it. Moving a task
    /// into the column it is in reorders it there.
    pub fn move_task(
        &self,
        id: &str,
        status: TaskStatus,
        before: Option<&str>,
        at: i64,
    ) -> Result<Task> {
        let id = self.task_id(id)?;
        let before = before.map(|before| self.task_id(before)).transpose()?;
        self.write(StoreChange::Tasks, |tx| {
            let mut task = row_of(tx, &id)?;
            let key = task.scope.key();
            task.position = match before.as_deref().filter(|before| *before != id) {
                Some(before) => above(tx, &key, status, before, &id)?,
                None => bottom(tx, &key, status, Some(&id))?,
            };
            if status != task.status {
                task.status = status;
                task.closed_at = status.is_closed().then_some(at);
            }
            task.updated_at = at;
            write_task(tx, &task)?;
            Ok(task)
        })
    }

    /// Removes a task for good and returns it.
    pub fn delete_task(&self, id: &str) -> Result<Task> {
        let id = self.task_id(id)?;
        self.write(StoreChange::Tasks, |tx| {
            let task = row_of(tx, &id)?;
            tx.execute("DELETE FROM task WHERE id = ?1", [&id])?;
            // The issue it was linked to stays linked to nothing: a sync
            // must not bring it back as a new task.
            tx.execute(
                "UPDATE task_link SET task_id = NULL WHERE task_id = ?1",
                [&id],
            )?;
            // Its steps are steps of nothing now, and nothing waits for it.
            tx.execute("UPDATE task SET parent = NULL WHERE parent = ?1", [&id])?;
            tx.execute(
                "UPDATE task SET needs = trim(replace(char(10) || needs || char(10),
                                                      char(10) || ?1 || char(10), char(10)),
                                              char(10))
                 WHERE instr(char(10) || needs || char(10), char(10) || ?1 || char(10)) > 0",
                [&id],
            )?;
            Ok(task)
        })
    }

    /// The task with this id, or with an id that ends in it (see
    /// [`Task::short_id`]).
    pub fn task(&self, id: &str) -> Result<Task> {
        let id = self.task_id(id)?;
        self.read(|connection| row_of(connection, &id))
    }

    /// The whole id that `given` names: itself, or the one id that ends in it
    /// when it is at least [`MIN_ID_TAIL`] characters of one.
    fn task_id(&self, given: &str) -> Result<String> {
        let given = given.trim().to_ascii_lowercase();
        let plain = !given.is_empty() && given.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !plain {
            return Err(StoreError::NotFound("task"));
        }
        self.read(|connection| {
            let exact: Option<String> = connection
                .query_row("SELECT id FROM task WHERE id = ?1", [&given], |row| {
                    row.get(0)
                })
                .optional()?;
            if let Some(id) = exact {
                return Ok(id);
            }
            if given.len() < MIN_ID_TAIL {
                return Err(StoreError::NotFound("task"));
            }
            let mut statement =
                connection.prepare_cached("SELECT id FROM task WHERE id LIKE '%' || ?1 LIMIT 2")?;
            let found: Vec<String> = statement
                .query_map([&given], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            match found.as_slice() {
                [] => Err(StoreError::NotFound("task")),
                [id] => Ok(id.clone()),
                _ => Err(TaskError::AmbiguousId(given.clone()).into()),
            }
        })
    }

    /// The tasks of one scope that `filter` keeps, in the order of the board:
    /// by column, then by position. At most `limit`, never more than
    /// [`MAX_LISTED`].
    pub fn tasks(
        &self,
        scope: &MemoryScope,
        filter: &TaskFilter,
        limit: usize,
    ) -> Result<Vec<Task>> {
        let key = scope.key();
        let status = filter.status.map(TaskStatus::as_str);
        let assignee = filter.assignee.as_deref().map(str::to_lowercase);
        let label = filter.label.as_deref().map(str::to_lowercase);
        self.read(|connection| {
            let mut statement = connection.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM task t
                 WHERE t.scope = ?1
                   AND (?2 IS NULL OR t.status = ?2)
                   AND (?3 = 0 OR t.closed_at IS NULL)
                   AND (?4 IS NULL OR lower(t.assignee) = ?4)
                   AND (?5 IS NULL OR instr(char(10) || t.labels || char(10),
                                            char(10) || ?5 || char(10)) > 0)
                   AND (?7 IS NULL OR t.parent = ?7)
                 ORDER BY {}, t.position, t.pk
                 LIMIT ?6",
                status_order()
            ))?;
            let found = statement
                .query_map(
                    params![
                        key,
                        status,
                        filter.open_only,
                        assignee,
                        label,
                        sql_limit(limit.min(MAX_LISTED)),
                        filter.parent
                    ],
                    task_from_row,
                )?
                .collect::<rusqlite::Result<_>>()?;
            Ok(found)
        })
    }

    /// How many tasks each column of a scope holds, in the order of the
    /// board; every column is there, empty ones with 0.
    pub fn task_counts(&self, scope: &MemoryScope) -> Result<Vec<(TaskStatus, usize)>> {
        let key = scope.key();
        self.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT status, count(*) FROM task WHERE scope = ?1 GROUP BY 1")?;
            let rows: Vec<(String, i64)> = statement
                .query_map([&key], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(TaskStatus::ALL
                .iter()
                .map(|status| {
                    let held = rows
                        .iter()
                        .find(|(name, _)| name == status.as_str())
                        .map_or(0, |(_, held)| usize::try_from(*held).unwrap_or_default());
                    (*status, held)
                })
                .collect())
        })
    }

    /// Searches the titles and bodies of the tasks of a scope, best match
    /// first. A project is searched together with the global scope.
    pub fn search_tasks(
        &self,
        scope: &MemoryScope,
        words: &str,
        limit: usize,
    ) -> Result<Vec<TaskHit>> {
        let Some(expression) = fts_query(words) else {
            return Ok(Vec::new());
        };
        let key = scope.key();
        let (start, end) = (SNIPPET_START.to_string(), SNIPPET_END.to_string());
        self.read(|connection| {
            let mut statement = connection.prepare_cached(&format!(
                "SELECT {COLUMNS},
                        snippet(task_fts, 1, ?3, ?4, ?5, {SNIPPET_WORDS}),
                        task_fts.rank
                 FROM task_fts
                 JOIN task t ON t.pk = task_fts.rowid
                 WHERE task_fts MATCH ?1 AND t.scope IN (?2, '{GLOBAL_SCOPE}')
                 ORDER BY task_fts.rank, t.updated_at DESC
                 LIMIT ?6"
            ))?;
            let hits = statement
                .query_map(
                    params![
                        expression,
                        key,
                        start,
                        end,
                        SNIPPET_ELLIPSIS,
                        sql_limit(limit.min(MAX_LISTED))
                    ],
                    |row| {
                        Ok(TaskHit {
                            task: task_from_row(row)?,
                            snippet: row.get(COLUMN_COUNT)?,
                            rank: row.get(COLUMN_COUNT + 1)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<_>>()?;
            Ok(hits)
        })
    }
}

/// Writes back everything an update or a move may have changed.
fn write_task(tx: &Transaction<'_>, task: &Task) -> Result<()> {
    tx.execute(
        "UPDATE task SET title = ?2, body = ?3, status = ?4, priority = ?5, position = ?6,
                labels = ?7, assignee = ?8, title_hash = ?9, updated_at = ?10, closed_at = ?11,
                branch = ?12, parent = ?13, needs = ?14
         WHERE id = ?1",
        params![
            task.id,
            task.title,
            task.body,
            task.status.as_str(),
            task.priority.level(),
            task.position,
            task.labels.join("\n"),
            task.assignee,
            text_hash(&task.title),
            task.updated_at,
            task.closed_at,
            task.branch,
            task.parent,
            task.needs.join("\n")
        ],
    )?;
    Ok(())
}

/// Refuses a parent on another board, or one that is the task or one of
/// its steps (however far down).
fn check_parent(tx: &Transaction<'_>, task: &Task, parent: &str) -> Result<()> {
    let refused = || StoreError::from(TaskError::BadParent(parent.to_owned()));
    let mut at = Some(parent.to_owned());
    let mut seen = 0;
    while let Some(id) = at {
        if id == task.id || seen > MAX_LISTED {
            return Err(refused());
        }
        let found = row_of(tx, &id).map_err(|_| refused())?;
        if found.scope != task.scope {
            return Err(refused());
        }
        at = found.parent;
        seen += 1;
    }
    Ok(())
}

/// Refuses a need on another board, or one that waits, however far down,
/// for the task itself.
fn check_need(tx: &Transaction<'_>, task: &Task, need: &str) -> Result<()> {
    let refused = || StoreError::from(TaskError::BadNeed(need.to_owned()));
    let mut open = vec![need.to_owned()];
    let mut seen: Vec<String> = Vec::new();
    while let Some(id) = open.pop() {
        if id == task.id {
            return Err(refused());
        }
        if seen.contains(&id) {
            continue;
        }
        if seen.len() > MAX_LISTED {
            return Err(refused());
        }
        let found = row_of(tx, &id).map_err(|_| refused())?;
        if found.scope != task.scope {
            return Err(refused());
        }
        open.extend(found.needs);
        seen.push(id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api() -> MemoryScope {
        MemoryScope::project("/srv/api")
    }

    fn add(store: &Store, title: &str, at: i64) -> Task {
        store
            .add_task(&NewTask::titled(api(), title), at)
            .unwrap()
            .into_task()
    }

    fn titles(tasks: &[Task]) -> Vec<&str> {
        tasks.iter().map(|task| task.title.as_str()).collect()
    }

    #[test]
    fn a_task_is_kept_as_it_was_written() {
        let store = Store::open_in_memory().unwrap();
        let mut changes = store.subscribe();
        let saved = store
            .add_task(
                &NewTask {
                    scope: api(),
                    title: "  Ship the\nkanban  ".into(),
                    body: "Phase 1.\r\nStore first.".into(),
                    status: Some(TaskStatus::Backlog),
                    priority: Some(TaskPriority::High),
                    labels: vec!["Backend".into(), "backend".into(), " Needs Review ".into()],
                    assignee: Some(" Claude ".into()),
                    agent: Some("Codex".into()),
                },
                1_000,
            )
            .unwrap();
        assert_eq!(changes.try_next(), Some(StoreChange::Tasks));
        let task = saved.into_task();
        assert_eq!(task.title, "Ship the kanban");
        assert_eq!(task.body, "Phase 1.\nStore first.");
        assert_eq!(task.labels, ["backend", "needs review"]);
        assert_eq!(task.assignee.as_deref(), Some("Claude"));
        assert_eq!(task.agent.as_deref(), Some("codex"));
        assert_eq!((task.created_at, task.updated_at), (1_000, 1_000));
        assert_eq!(task.closed_at, None);
        assert_eq!(store.task(&task.id).unwrap(), task);
        assert_eq!(store.task(task.short_id()).unwrap(), task);
    }

    #[test]
    fn a_task_has_steps_that_wait_for_each_other_and_never_in_a_circle() {
        let store = Store::open_in_memory().unwrap();
        let goal = add(&store, "Ship billing", 1);
        let schema = add(&store, "Billing schema", 1);
        let api = add(&store, "Billing API", 1);
        let page = add(&store, "Billing page", 1);
        let step = |needs: &[&Task]| TaskPatch {
            parent: Some(Some(goal.short_id().to_owned())),
            needs: Some(
                needs
                    .iter()
                    .map(|need| need.short_id().to_owned())
                    .collect(),
            ),
            ..TaskPatch::default()
        };
        store.update_task(&schema.id, &step(&[]), 2).unwrap();
        let api = store.update_task(&api.id, &step(&[&schema]), 2).unwrap();
        assert_eq!(api.parent.as_deref(), Some(goal.id.as_str()));
        assert_eq!(api.needs, std::slice::from_ref(&schema.id));
        store
            .update_task(&page.id, &step(&[&api, &schema]), 2)
            .unwrap();
        let steps = TaskFilter {
            parent: Some(goal.id.clone()),
            ..TaskFilter::default()
        };
        assert_eq!(
            titles(
                &store
                    .tasks(&MemoryScope::project("/srv/api"), &steps, 10)
                    .unwrap()
            ),
            ["Billing schema", "Billing API", "Billing page"]
        );

        // Needs never go round, and a task is never its own ancestor.
        let circle = TaskPatch {
            needs: Some(vec![page.id.clone()]),
            ..TaskPatch::default()
        };
        assert!(store.update_task(&schema.id, &circle, 3).is_err());
        let own = TaskPatch {
            parent: Some(Some(schema.id.clone())),
            ..TaskPatch::default()
        };
        assert!(store.update_task(&goal.id, &own, 3).is_err());
        let elsewhere = store
            .add_task(&NewTask::titled(MemoryScope::Global, "Elsewhere"), 1)
            .unwrap()
            .into_task();
        let across = TaskPatch {
            needs: Some(vec![elsewhere.id.clone()]),
            ..TaskPatch::default()
        };
        assert!(store.update_task(&page.id, &across, 3).is_err());

        // A deleted task is a parent and a need of nothing.
        store.delete_task(&api.id).unwrap();
        assert_eq!(
            store.task(&page.id).unwrap().needs,
            std::slice::from_ref(&schema.id)
        );
        store.delete_task(&goal.id).unwrap();
        assert_eq!(store.task(&schema.id).unwrap().parent, None);
    }

    #[test]
    fn the_same_open_title_is_one_task_and_a_closed_one_does_not_count() {
        let store = Store::open_in_memory().unwrap();
        let first = add(&store, "Fix the login redirect", 1);
        let again = store
            .add_task(&NewTask::titled(api(), "  fix the LOGIN redirect. "), 2)
            .unwrap();
        assert_eq!(again, SavedTask::Known(first.clone()));
        assert_eq!(
            store
                .tasks(&api(), &TaskFilter::default(), 10)
                .unwrap()
                .len(),
            1
        );

        let patch = TaskPatch {
            status: Some(TaskStatus::Done),
            ..TaskPatch::default()
        };
        store.update_task(&first.id, &patch, 3).unwrap();
        let back = store
            .add_task(&NewTask::titled(api(), "Fix the login redirect"), 4)
            .unwrap();
        assert!(matches!(back, SavedTask::New(_)));
        // Another project's board is another board.
        let other = store
            .add_task(
                &NewTask::titled(MemoryScope::project("/srv/web"), "Fix the login redirect"),
                5,
            )
            .unwrap();
        assert!(matches!(other, SavedTask::New(_)));
    }

    #[test]
    fn what_is_empty_or_too_long_is_refused_and_nothing_is_written() {
        let store = Store::open_in_memory().unwrap();
        let mut changes = store.subscribe();
        let refused = |new: NewTask| store.add_task(&new, 1).unwrap_err().to_string();
        assert!(refused(NewTask::titled(api(), " \n ")).contains("needs a title"));
        assert!(
            refused(NewTask::titled(api(), "x".repeat(MAX_TITLE_CHARS + 1))).contains("title has")
        );
        let mut body = NewTask::titled(api(), "ok");
        body.body = "y".repeat(MAX_BODY_CHARS + 1);
        assert!(refused(body).contains("body has"));
        let mut labels = NewTask::titled(api(), "ok");
        labels.labels = (0..=MAX_LABELS).map(|n| format!("l{n}")).collect();
        assert!(refused(labels).contains("at most"));
        let mut blank = NewTask::titled(api(), "ok");
        blank.labels = vec!["  ".into()];
        assert!(refused(blank).contains("not a label"));
        assert!(
            refused(NewTask::titled(MemoryScope::project("relative/dir"), "ok"))
                .contains("absolute")
        );
        assert_eq!(changes.try_next(), None);
        assert!(store
            .tasks(&api(), &TaskFilter::default(), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_task_lands_at_the_bottom_of_its_column_and_moves_above_another() {
        let store = Store::open_in_memory().unwrap();
        let a = add(&store, "A", 1);
        let b = add(&store, "B", 2);
        let c = add(&store, "C", 3);
        let board = |store: &Store, status| {
            store
                .tasks(
                    &api(),
                    &TaskFilter {
                        status: Some(status),
                        ..TaskFilter::default()
                    },
                    10,
                )
                .unwrap()
        };
        assert_eq!(titles(&board(&store, TaskStatus::Todo)), ["A", "B", "C"]);

        // Above the first, between two, and onto itself (nothing changes).
        store
            .move_task(&c.id, TaskStatus::Todo, Some(&a.id), 4)
            .unwrap();
        assert_eq!(titles(&board(&store, TaskStatus::Todo)), ["C", "A", "B"]);
        store
            .move_task(&b.id, TaskStatus::Todo, Some(&a.id), 5)
            .unwrap();
        assert_eq!(titles(&board(&store, TaskStatus::Todo)), ["C", "B", "A"]);
        store
            .move_task(&b.id, TaskStatus::Todo, Some(&b.id), 6)
            .unwrap();
        assert_eq!(titles(&board(&store, TaskStatus::Todo)), ["C", "A", "B"]);

        // Into another column: the bottom of it, and the closing time follows.
        let doing = store.move_task(&a.id, TaskStatus::Doing, None, 7).unwrap();
        assert_eq!((doing.status, doing.closed_at), (TaskStatus::Doing, None));
        let done = store.move_task(&a.id, TaskStatus::Done, None, 8).unwrap();
        assert_eq!(done.closed_at, Some(8));
        let reopened = store.move_task(&a.id, TaskStatus::Todo, None, 9).unwrap();
        assert_eq!(reopened.closed_at, None);
        assert_eq!(titles(&board(&store, TaskStatus::Todo)), ["C", "B", "A"]);
    }

    #[test]
    fn an_update_changes_only_what_it_names() {
        let store = Store::open_in_memory().unwrap();
        let mut new = NewTask::titled(api(), "Write docs");
        new.labels = vec!["docs".into()];
        new.assignee = Some("ana".into());
        let task = store.add_task(&new, 1).unwrap().into_task();

        let changed = store
            .update_task(
                &task.id,
                &TaskPatch {
                    priority: Some(TaskPriority::Urgent),
                    ..TaskPatch::default()
                },
                2,
            )
            .unwrap();
        assert_eq!(changed.priority, TaskPriority::Urgent);
        assert_eq!(changed.labels, ["docs"]);
        assert_eq!(changed.assignee.as_deref(), Some("ana"));
        assert_eq!((changed.created_at, changed.updated_at), (1, 2));

        let cleared = store
            .update_task(
                &task.id,
                &TaskPatch {
                    assignee: Some(None),
                    labels: Some(Vec::new()),
                    ..TaskPatch::default()
                },
                3,
            )
            .unwrap();
        assert_eq!((cleared.assignee, cleared.labels.len()), (None, 0));

        let nothing = store.update_task(&task.id, &TaskPatch::default(), 4);
        assert!(nothing
            .unwrap_err()
            .to_string()
            .contains("nothing to change"));
    }

    #[test]
    fn the_branch_of_a_task_is_kept_and_taken_away_and_is_one_plain_word() {
        let store = Store::open_in_memory().unwrap();
        let task = add(&store, "Fix the login", 1);
        assert_eq!(task.branch, None);
        let set = |branch: Option<&str>| TaskPatch {
            branch: Some(branch.map(str::to_owned)),
            ..TaskPatch::default()
        };
        let started = store
            .update_task(&task.id, &set(Some(" fix-the-login-1a2b3c4d ")), 2)
            .unwrap();
        assert_eq!(started.branch.as_deref(), Some("fix-the-login-1a2b3c4d"));
        // It survives another change, and is read back whole.
        let other = TaskPatch {
            priority: Some(TaskPriority::High),
            ..TaskPatch::default()
        };
        store.update_task(&task.id, &other, 3).unwrap();
        assert_eq!(
            store.task(&task.id).unwrap().branch.as_deref(),
            Some("fix-the-login-1a2b3c4d")
        );
        let refused = store.update_task(&task.id, &set(Some("two words")), 4);
        assert!(refused.unwrap_err().to_string().contains("not a branch"));
        let cleared = store.update_task(&task.id, &set(None), 5).unwrap();
        assert_eq!(cleared.branch, None);
    }

    #[test]
    fn listing_filters_by_column_assignee_label_and_openness() {
        let store = Store::open_in_memory().unwrap();
        let mut one = NewTask::titled(api(), "One");
        one.assignee = Some("Ana".into());
        one.labels = vec!["bug".into()];
        store.add_task(&one, 1).unwrap();
        let mut two = NewTask::titled(api(), "Two");
        two.status = Some(TaskStatus::Done);
        two.labels = vec!["bug".into(), "ui".into()];
        store.add_task(&two, 2).unwrap();
        let mut three = NewTask::titled(api(), "Three");
        three.status = Some(TaskStatus::Doing);
        store.add_task(&three, 3).unwrap();

        let list = |filter: TaskFilter| {
            let found = store.tasks(&api(), &filter, 10).unwrap();
            found.into_iter().map(|task| task.title).collect::<Vec<_>>()
        };
        // Column order first: todo, doing, done.
        assert_eq!(list(TaskFilter::default()), ["One", "Three", "Two"]);
        assert_eq!(
            list(TaskFilter {
                open_only: true,
                ..TaskFilter::default()
            }),
            ["One", "Three"]
        );
        assert_eq!(
            list(TaskFilter {
                assignee: Some("ANA".into()),
                ..TaskFilter::default()
            }),
            ["One"]
        );
        assert_eq!(
            list(TaskFilter {
                label: Some("ui".into()),
                ..TaskFilter::default()
            }),
            ["Two"]
        );
        // A label is matched whole, not as a part of another.
        assert!(list(TaskFilter {
            label: Some("u".into()),
            ..TaskFilter::default()
        })
        .is_empty());
        let counts = store.task_counts(&api()).unwrap();
        assert_eq!(counts[1], (TaskStatus::Todo, 1));
        assert_eq!(counts[2], (TaskStatus::Doing, 1));
        assert_eq!(counts[4], (TaskStatus::Done, 1));
        assert_eq!(counts[0], (TaskStatus::Backlog, 0));
    }

    #[test]
    fn search_finds_titles_and_bodies_and_the_global_board() {
        let store = Store::open_in_memory().unwrap();
        let mut one = NewTask::titled(api(), "Rotate the signing key");
        one.body = "The old key expires in March.".into();
        store.add_task(&one, 1).unwrap();
        store
            .add_task(
                &NewTask::titled(MemoryScope::Global, "Renew the domain key"),
                2,
            )
            .unwrap();
        store
            .add_task(
                &NewTask::titled(MemoryScope::project("/srv/web"), "Another key"),
                3,
            )
            .unwrap();
        let found = store.search_tasks(&api(), "key", 10).unwrap();
        let mut found: Vec<_> = found.iter().map(|hit| hit.task.title.as_str()).collect();
        found.sort_unstable();
        assert_eq!(found, ["Renew the domain key", "Rotate the signing key"]);
        assert!(store.search_tasks(&api(), "   ", 10).unwrap().is_empty());
    }

    #[test]
    fn a_deleted_task_is_gone_and_a_short_id_must_be_enough_and_unique() {
        let store = Store::open_in_memory().unwrap();
        let task = add(&store, "Temporary", 1);
        assert!(store.task("abc").is_err());
        let gone = store.delete_task(task.short_id()).unwrap();
        assert_eq!(gone.id, task.id);
        assert!(matches!(
            store.task(&task.id),
            Err(StoreError::NotFound("task"))
        ));
        assert!(store
            .search_tasks(&api(), "temporary", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn statuses_and_priorities_are_read_in_the_trackers_words() {
        for (word, status) in [
            ("In Progress", TaskStatus::Doing),
            ("in_review", TaskStatus::Review),
            ("Closed", TaskStatus::Done),
            ("cancelled", TaskStatus::Canceled),
            ("unstarted", TaskStatus::Todo),
        ] {
            assert_eq!(TaskStatus::parse(word).unwrap(), status, "{word}");
        }
        assert!(TaskStatus::parse("later").is_err());
        assert_eq!(TaskPriority::parse("3").unwrap(), TaskPriority::High);
        assert_eq!(TaskPriority::parse("Urgent").unwrap(), TaskPriority::Urgent);
        assert!(TaskPriority::parse("5").is_err());
        assert_eq!(TaskPriority::from_level(99), TaskPriority::None);
    }
}
