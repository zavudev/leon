//! The board of one project, as the command line and the MCP server see it.
//!
//! [`Service`] is the store seen from one folder: it knows the project's root
//! (resolved as the shared memory resolves it, so a worktree and the main
//! checkout share one board) and the clock, and nothing else. What an answer
//! looks like, as JSON for a program and as lines for a person, is here too
//! ([`json_of`], [`line`], [`detail`], [`board`]), so that the two front ends
//! say the same thing.

use std::sync::Arc;

use leon_core::{
    MemoryScope, NewTask, SavedTask, Store, StoreError, Task, TaskFilter, TaskHit, TaskPatch,
    TaskStatus,
};
use serde_json::{json, Value};

use crate::task_mcp::{Backend, Draft, Reach};

/// The store seen from one project's folder.
pub struct Service {
    store: Arc<Store>,
    root: String,
    now: fn() -> i64,
    /// Every board is this service's, not only its project's and the
    /// global one: King Leon's.
    anywhere: bool,
}

impl Service {
    /// The service of the project rooted at `root`.
    pub fn new(store: Arc<Store>, root: String) -> Self {
        Self {
            store,
            root,
            now: crate::memory::now_millis,
            anywhere: false,
        }
    }

    /// The same, seeing every board when `anywhere`.
    pub fn anywhere(mut self, anywhere: bool) -> Self {
        self.anywhere = anywhere;
        self
    }

    /// The board a call of the tools means.
    fn board_of(&self, reach: &Reach) -> MemoryScope {
        match reach {
            Reach::Global => MemoryScope::Global,
            Reach::Project => self.scope(false),
            Reach::Root(root) => MemoryScope::project(root.clone()),
        }
    }

    /// The same with another clock.
    #[cfg(test)]
    pub fn at(mut self, now: fn() -> i64) -> Self {
        self.now = now;
        self
    }

    /// The project's root.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// The scope a command means.
    pub fn scope(&self, global: bool) -> MemoryScope {
        if global {
            MemoryScope::Global
        } else {
            MemoryScope::project(self.root.clone())
        }
    }

    /// Creates a task, or answers the open one that has the same title.
    pub fn add(&self, new: &NewTask) -> Result<SavedTask, StoreError> {
        self.store.add_task(new, (self.now)())
    }

    /// Changes a task.
    pub fn update(&self, id: &str, patch: &TaskPatch) -> Result<Task, StoreError> {
        self.store.update_task(id, patch, (self.now)())
    }

    /// Moves a task to a column, above `before` when that is given.
    pub fn move_to(
        &self,
        id: &str,
        status: TaskStatus,
        before: Option<&str>,
    ) -> Result<Task, StoreError> {
        self.store.move_task(id, status, before, (self.now)())
    }

    /// Removes a task for good.
    pub fn delete(&self, id: &str) -> Result<Task, StoreError> {
        self.store.delete_task(id)
    }

    /// One task, whole, by its id or the end of it. Only a task of this
    /// project or a global one: another project's id is not found.
    pub fn get(&self, id: &str) -> Result<Task, StoreError> {
        let task = self.store.task(id)?;
        let here = match &task.scope {
            _ if self.anywhere => true,
            MemoryScope::Global => true,
            MemoryScope::Project(root) => {
                leon_core::path::key(root) == leon_core::path::key(&self.root)
            }
        };
        if here {
            Ok(task)
        } else {
            Err(StoreError::NotFound("task"))
        }
    }

    /// The tasks of this project, or of the global board, that `filter` keeps.
    pub fn list(
        &self,
        global: bool,
        filter: &TaskFilter,
        limit: usize,
    ) -> Result<Vec<Task>, StoreError> {
        self.store.tasks(&self.scope(global), filter, limit)
    }

    /// The whole board of this project, or the global one, a column at a
    /// time (see [`load_board`]).
    pub fn board(&self, global: bool, per_column: usize) -> Result<Vec<Task>, StoreError> {
        load_board(&self.store, &self.scope(global), per_column)
    }

    /// The best matches for `words` in this project and the global board.
    pub fn search(
        &self,
        global: bool,
        words: &str,
        limit: usize,
    ) -> Result<Vec<TaskHit>, StoreError> {
        self.store.search_tasks(&self.scope(global), words, limit)
    }
}

/// The tasks of a board, every column read on its own and at most
/// `per_column` of each, in the order of the board. Reading the board as one
/// list cut at a limit would let a long backlog hide `doing` and `done`.
pub fn load_board(
    store: &Store,
    scope: &MemoryScope,
    per_column: usize,
) -> Result<Vec<Task>, StoreError> {
    let mut all = Vec::new();
    for status in TaskStatus::ALL {
        let filter = TaskFilter {
            status: Some(status),
            ..TaskFilter::default()
        };
        all.extend(store.tasks(scope, &filter, per_column)?);
    }
    Ok(all)
}

/// What an error says to the person or agent that caused it.
pub fn sentence(error: &StoreError) -> String {
    match error {
        StoreError::NotFound(_) => "no task of this board has that id".to_owned(),
        StoreError::Invalid(why) => why.clone(),
        other => other.to_string(),
    }
}

/// A task as JSON: every field, the times in milliseconds.
pub fn json_of(task: &Task, snippet: Option<&str>) -> Value {
    let mut value = json!({
        "id": task.id,
        "short_id": task.short_id(),
        "scope": match task.scope {
            MemoryScope::Global => "global",
            MemoryScope::Project(_) => "project",
        },
        // Whose board: what an agent that sees every board names it by.
        "project": match &task.scope {
            MemoryScope::Global => None,
            MemoryScope::Project(root) => Some(root.clone()),
        },
        "title": task.title,
        "body": task.body,
        "status": task.status.as_str(),
        "priority": task.priority.as_str(),
        "labels": task.labels,
        "assignee": task.assignee,
        "agent": task.agent,
        "position": task.position,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
        "closed_at": task.closed_at,
        "branch": task.branch,
    });
    if let (Some(snippet), Some(object)) = (snippet, value.as_object_mut()) {
        let plain: String = snippet
            .chars()
            .filter(|c| *c != leon_core::SNIPPET_START && *c != leon_core::SNIPPET_END)
            .collect();
        object.insert("snippet".into(), Value::String(plain));
    }
    value
}

/// One task on one line: `1a2b3c4d  todo  high  Title  @ana #bug #ui`.
pub fn line(task: &Task) -> String {
    let mut line = format!("{}  {:<8}", task.short_id(), task.status.as_str());
    if task.priority != leon_core::TaskPriority::None {
        line.push_str(&format!("  {}", task.priority.as_str()));
    }
    line.push_str(&format!("  {}", task.title));
    if let Some(assignee) = &task.assignee {
        line.push_str(&format!("  @{assignee}"));
    }
    for label in &task.labels {
        line.push_str(&format!("  #{}", label.replace(' ', "-")));
    }
    line
}

/// A task in full, for `show`.
pub fn detail(task: &Task) -> String {
    let mut text = format!("{}\n", task.title);
    let mut field = |name: &str, value: String| text.push_str(&format!("{name:<10}{value}\n"));
    field("id", task.id.clone());
    field("status", task.status.as_str().to_owned());
    field("priority", task.priority.as_str().to_owned());
    if let Some(assignee) = &task.assignee {
        field("assignee", assignee.clone());
    }
    if !task.labels.is_empty() {
        field("labels", task.labels.join(", "));
    }
    if let Some(agent) = &task.agent {
        field("created by", agent.clone());
    }
    if let Some(branch) = &task.branch {
        field("branch", branch.clone());
    }
    if !task.body.is_empty() {
        text.push('\n');
        text.push_str(&task.body);
        text.push('\n');
    }
    text
}

/// The board as text: a heading per column that holds a task, the open
/// columns always, and a line per task.
pub fn board(tasks: &[Task]) -> String {
    let mut text = String::new();
    for status in TaskStatus::ALL {
        let column: Vec<&Task> = tasks.iter().filter(|task| task.status == status).collect();
        if column.is_empty() && status.is_closed() {
            continue;
        }
        text.push_str(&format!(
            "{} ({})\n",
            status.as_str().to_uppercase(),
            column.len()
        ));
        for task in column {
            text.push_str(&format!("  {}\n", line(task)));
        }
    }
    text
}

impl Backend for Service {
    fn add(&mut self, draft: Draft) -> Result<SavedTask, String> {
        let new = NewTask {
            scope: self.board_of(&draft.reach),
            title: draft.title,
            body: draft.body,
            status: draft.status,
            priority: draft.priority,
            labels: draft.labels,
            assignee: draft.assignee,
            agent: draft.agent,
        };
        Service::add(self, &new).map_err(|e| sentence(&e))
    }

    fn list(
        &mut self,
        reach: Reach,
        filter: TaskFilter,
        limit: usize,
    ) -> Result<Vec<Task>, String> {
        self.store
            .tasks(&self.board_of(&reach), &filter, limit)
            .map_err(|e| sentence(&e))
    }

    fn get(&mut self, id: &str) -> Result<Task, String> {
        Service::get(self, id).map_err(|e| sentence(&e))
    }

    fn update(&mut self, id: &str, patch: TaskPatch) -> Result<Task, String> {
        Service::get(self, id).map_err(|e| sentence(&e))?;
        Service::update(self, id, &patch).map_err(|e| sentence(&e))
    }

    fn move_to(
        &mut self,
        id: &str,
        status: TaskStatus,
        before: Option<&str>,
    ) -> Result<Task, String> {
        Service::get(self, id).map_err(|e| sentence(&e))?;
        Service::move_to(self, id, status, before).map_err(|e| sentence(&e))
    }

    fn search(&mut self, words: &str, reach: Reach, limit: usize) -> Result<Vec<TaskHit>, String> {
        self.store
            .search_tasks(&self.board_of(&reach), words, limit)
            .map_err(|e| sentence(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(root: &str) -> Service {
        Service::new(Store::open_in_memory().unwrap(), root.to_owned()).at(|| 5_000)
    }

    #[test]
    fn a_task_of_another_project_is_not_found_from_this_one() {
        let store = Store::open_in_memory().unwrap();
        let api = Service::new(store.clone(), "/srv/api".to_owned()).at(|| 1);
        let web = Service::new(store, "/srv/web".to_owned()).at(|| 1);
        let task = api
            .add(&NewTask::titled(api.scope(false), "Private to api"))
            .unwrap()
            .into_task();
        assert!(api.get(&task.id).is_ok());
        assert!(matches!(
            web.get(&task.id),
            Err(StoreError::NotFound("task"))
        ));
        // A global task is everybody's.
        let shared = api
            .add(&NewTask::titled(api.scope(true), "For every project"))
            .unwrap()
            .into_task();
        assert!(web.get(&shared.id).is_ok());
    }

    #[test]
    fn a_task_reads_as_a_line_and_as_json() {
        let service = service("/srv/api");
        let mut new = NewTask::titled(service.scope(false), "Fix login");
        new.priority = Some(leon_core::TaskPriority::High);
        new.assignee = Some("ana".into());
        new.labels = vec!["bug".into(), "needs review".into()];
        let task = service.add(&new).unwrap().into_task();
        assert_eq!(
            line(&task),
            format!(
                "{}  todo      high  Fix login  @ana  #bug  #needs-review",
                task.short_id()
            )
        );
        let value = json_of(&task, Some("a \u{2}match\u{3} here"));
        assert_eq!(value["status"], "todo");
        assert_eq!(value["priority"], "high");
        assert_eq!(value["labels"][1], "needs review");
        assert_eq!(value["created_at"], 5_000);
        assert_eq!(value["snippet"], "a match here");
    }

    #[test]
    fn a_long_backlog_does_not_hide_the_other_columns() {
        let store = Store::open_in_memory().unwrap();
        let service = Service::new(store, "/srv/api".to_owned()).at(|| 1);
        let scope = service.scope(false);
        for n in 0..6 {
            let mut new = NewTask::titled(scope.clone(), format!("Backlog {n}"));
            new.status = Some(TaskStatus::Backlog);
            service.add(&new).unwrap();
        }
        let mut doing = NewTask::titled(scope.clone(), "In flight");
        doing.status = Some(TaskStatus::Doing);
        service.add(&doing).unwrap();
        // One list cut at 3 shows only backlog; the board reads each column.
        let flat = service.list(false, &TaskFilter::default(), 3).unwrap();
        assert!(flat.iter().all(|task| task.status == TaskStatus::Backlog));
        let board = service.board(false, 3).unwrap();
        let backlog = board
            .iter()
            .filter(|t| t.status == TaskStatus::Backlog)
            .count();
        assert_eq!(backlog, 3);
        assert!(board.iter().any(|task| task.title == "In flight"));
    }

    #[test]
    fn the_board_lists_the_open_columns_and_only_the_closed_ones_that_hold_tasks() {
        let service = service("/srv/api");
        let scope = service.scope(false);
        service
            .add(&NewTask::titled(scope.clone(), "First"))
            .unwrap();
        let text = board(&service.list(false, &TaskFilter::default(), 10).unwrap());
        assert!(text.contains("BACKLOG (0)"), "{text}");
        assert!(text.contains("TODO (1)\n  "), "{text}");
        assert!(text.contains("DOING (0)"), "{text}");
        assert!(!text.contains("DONE"), "{text}");
    }
}
