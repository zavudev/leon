//! The MCP server of the board: one JSON-RPC message in, at most one out.
//!
//! An agent that speaks the Model Context Protocol starts `leon task mcp` and
//! talks to it over standard input and output, one JSON message per line (the
//! stdio transport), the way it does with `leon memory mcp`. [`Server::handle`]
//! is that conversation without the pipes; the store is behind [`Backend`], so
//! every exchange is tested without a database.
//!
//! The tools are [`TOOLS`]. They create, read, change and move tasks and
//! never delete one: an agent that no longer wants a task cancels it, and
//! removing for good is the person's (`leon task rm`). A tool that ran and
//! refused (a title that is too long, an id that names nothing) answers with
//! `isError`, as the specification asks, so the agent reads why and can
//! correct itself; a message that is not valid is a JSON-RPC error.

use leon_core::{SavedTask, Task, TaskFilter, TaskHit, TaskPatch, TaskPriority, TaskStatus};
use serde_json::{json, Map, Value};

use crate::tasks::json_of;

/// The versions of the protocol this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The name the server gives for itself.
pub const SERVER_NAME: &str = "leon-tasks";

/// The longest message that is read, in bytes.
pub const MAX_LINE: usize = 1 << 20;

/// The names of the tools, in the order they are listed.
pub const TOOLS: [&str; 6] = [
    "task_create",
    "task_list",
    "task_get",
    "task_update",
    "task_move",
    "task_search",
];

const DEFAULT_LISTED: usize = 50;
const MOST: usize = 200;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Which board a tool is asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// The project the server was started in.
    Project,
    /// The global board.
    Global,
    /// The project rooted at this folder, named by the agent.
    Root(String),
}

/// A task an agent asks to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// Where it goes.
    pub reach: Reach,
    /// What is to be done.
    pub title: String,
    /// The detail; may be empty.
    pub body: String,
    /// The column, when the agent said.
    pub status: Option<TaskStatus>,
    /// The priority, when the agent said.
    pub priority: Option<TaskPriority>,
    /// Its labels.
    pub labels: Vec<String>,
    /// Who is to do it, when the agent said.
    pub assignee: Option<String>,
    /// Who creates it: the client's name, when it gave one.
    pub agent: Option<String>,
}

/// What the server asks of the board. An `Err` is a sentence for the agent.
pub trait Backend {
    /// Creates a task, or answers the open one with the same title.
    fn add(&mut self, draft: Draft) -> Result<SavedTask, String>;
    /// The tasks `filter` keeps, in the order of the board.
    fn list(&mut self, reach: Reach, filter: TaskFilter, limit: usize)
        -> Result<Vec<Task>, String>;
    /// One task, whole.
    fn get(&mut self, id: &str) -> Result<Task, String>;
    /// Changes one task.
    fn update(&mut self, id: &str, patch: TaskPatch) -> Result<Task, String>;
    /// Moves one task to a column, above `before` when given.
    fn move_to(
        &mut self,
        id: &str,
        status: TaskStatus,
        before: Option<&str>,
    ) -> Result<Task, String>;
    /// The best matches for `words`.
    fn search(&mut self, words: &str, reach: Reach, limit: usize) -> Result<Vec<TaskHit>, String>;
}

/// The server: a backend, and what the client said of itself.
#[derive(Debug)]
pub struct Server<B> {
    backend: B,
    version: String,
    client: Option<String>,
}

/// A request that cannot be answered with a result.
struct Refusal {
    code: i64,
    message: String,
}

fn refuse(code: i64, message: impl Into<String>) -> Refusal {
    Refusal {
        code,
        message: message.into(),
    }
}

/// The tools as `tools/list` describes them.
pub fn tool_list() -> Value {
    let statuses: Vec<&str> = TaskStatus::ALL.iter().map(|s| s.as_str()).collect();
    let priorities: Vec<&str> = TaskPriority::ALL.iter().map(|p| p.as_str()).collect();
    let scope = json!({
        "type": "string",
        "enum": ["project", "global"],
        "default": "project",
        "description": "\"global\" for a task that is not of this project.",
    });
    let project = json!({
        "type": "string",
        "description": "The root folder of the project whose board it is, as the project list \
                        gives it. Only for an agent that runs in no project (King Leon); \
                        others leave it out.",
    });
    let id = json!({
        "type": "string",
        "minLength": 6,
        "description": "The id shown with the task (the eight characters are enough).",
    });
    let needs = json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "The ids of the tasks of the same board this one waits for: a run \
                        starts it once they are done. Replaces what it waited for.",
    });
    let parent = json!({
        "type": "string",
        "description": "The id of the task this one is a step of: a run of that task carries \
                        out its steps.",
    });
    let labels = json!({
        "type": "array",
        "items": { "type": "string", "maxLength": leon_core::store::tasks::MAX_LABEL_CHARS },
        "maxItems": leon_core::store::tasks::MAX_LABELS,
    });
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            },
        })
    };
    json!([
        tool(
            "task_create",
            "Add a task to the board of this project. Use it for work that is found and not \
             done now: a bug noticed, a follow-up, a step left. Creating a task whose title an \
             open task already has returns that task and adds nothing, so it is safe to retry.",
            json!({
                "title": {
                    "type": "string", "minLength": 1,
                    "maxLength": leon_core::store::tasks::MAX_TITLE_CHARS,
                    "description": "One line: what is to be done.",
                },
                "body": {
                    "type": "string",
                    "maxLength": leon_core::store::tasks::MAX_BODY_CHARS,
                    "description": "The detail: why, where, how to know it is done.",
                },
                "status": { "type": "string", "enum": statuses.clone(), "default": "todo" },
                "priority": { "type": "string", "enum": priorities.clone() },
                "labels": labels.clone(),
                "assignee": { "type": "string", "description": "A person or an agent." },
                "parent": parent.clone(),
                "needs": needs.clone(),
                "scope": scope.clone(),
                "project": project.clone(),
            }),
            &["title"],
        ),
        tool(
            "task_list",
            "List the tasks of this project's board in the order of the board (by column, \
             then by position). Closed ones are left out unless asked for.",
            json!({
                "status": { "type": "string", "enum": statuses.clone() },
                "include_closed": { "type": "boolean", "default": false },
                "assignee": { "type": "string" },
                "label": { "type": "string" },
                "parent": {
                    "type": "string",
                    "description": "Only the steps of this task, by its id.",
                },
                "scope": scope.clone(),
                "project": project.clone(),
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": MOST, "default": DEFAULT_LISTED,
                },
            }),
            &[],
        ),
        tool(
            "task_get",
            "Read one task in full, by its id: its whole body and every field.",
            json!({ "id": id.clone() }),
            &["id"],
        ),
        tool(
            "task_update",
            "Change a task: its title, body, status, priority, labels, assignee, parent or \
             needs. What is not given stays. A new status puts the task at the bottom of that \
             column.",
            json!({
                "id": id.clone(),
                "title": { "type": "string", "maxLength": leon_core::store::tasks::MAX_TITLE_CHARS },
                "body": { "type": "string", "maxLength": leon_core::store::tasks::MAX_BODY_CHARS },
                "status": { "type": "string", "enum": statuses.clone() },
                "priority": { "type": "string", "enum": priorities },
                "labels": labels,
                "assignee": {
                    "type": "string",
                    "description": "The new assignee; an empty string takes it away.",
                },
                "branch": {
                    "type": "string",
                    "description": "The branch of the worktree this task is worked on in; an \
                                    empty string takes it away.",
                },
                "parent": {
                    "type": "string",
                    "description": "The task this one is a step of; an empty string makes it \
                                    a step of nothing.",
                },
                "needs": needs,
            }),
            &["id"],
        ),
        tool(
            "task_move",
            "Move a task to a column, as on the board: to the bottom of it, or right above \
             another task of that column. Move a task to \"doing\" when you start it, to \
             \"review\" when it is ready to be looked at and to \"done\" when it is finished.",
            json!({
                "id": id,
                "status": { "type": "string", "enum": statuses },
                "before": {
                    "type": "string",
                    "description": "The id of the task to place it above, in that column.",
                },
            }),
            &["id", "status"],
        ),
        tool(
            "task_search",
            "Search the titles and bodies of the tasks of this project and the global board. \
             Use it before creating a task, to see whether it is there already.",
            json!({
                "query": { "type": "string", "minLength": 1 },
                "scope": scope,
                "project": project,
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": MOST, "default": 10,
                },
            }),
            &["query"],
        ),
    ])
}

/// The answer to a message longer than [`MAX_LINE`], which is not read.
pub fn too_long() -> String {
    json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": INVALID_REQUEST, "message": "the message is too long" },
    })
    .to_string()
}

/// A client's name as an agent tag; see [`leon_memory::mcp::client_tag`].
fn client_tag(name: &str) -> Option<String> {
    leon_memory::mcp::client_tag(name)
}

/// The arguments of a tool call, read with the errors a wrong one gets.
struct Arguments<'a>(&'a Map<String, Value>);

impl Arguments<'_> {
    fn text(&self, name: &str) -> Result<Option<String>, Refusal> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(refuse(
                INVALID_PARAMS,
                format!("\"{name}\" must be a string"),
            )),
        }
    }

    fn required(&self, name: &str) -> Result<String, Refusal> {
        self.text(name)?
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| refuse(INVALID_PARAMS, format!("\"{name}\" is required")))
    }

    fn reach(&self) -> Result<Reach, Refusal> {
        let project = self.text("project")?.filter(|root| !root.trim().is_empty());
        match (self.text("scope")?.as_deref(), project) {
            (Some("global"), Some(_)) => Err(refuse(
                INVALID_PARAMS,
                "\"project\" names a project's board; it cannot be given with \"global\"",
            )),
            (None | Some("project"), Some(root)) => Ok(Reach::Root(root)),
            (None | Some("project"), None) => Ok(Reach::Project),
            (Some("global"), None) => Ok(Reach::Global),
            (Some(other), _) => Err(refuse(
                INVALID_PARAMS,
                format!("\"scope\" must be \"project\" or \"global\", not {other:?}"),
            )),
        }
    }

    fn status(&self, name: &str) -> Result<Option<TaskStatus>, Refusal> {
        self.text(name)?
            .map(|word| TaskStatus::parse(&word))
            .transpose()
            .map_err(|error| refuse(INVALID_PARAMS, error.to_string()))
    }

    fn priority(&self) -> Result<Option<TaskPriority>, Refusal> {
        self.text("priority")?
            .map(|word| TaskPriority::parse(&word))
            .transpose()
            .map_err(|error| refuse(INVALID_PARAMS, error.to_string()))
    }

    fn labels(&self) -> Result<Option<Vec<String>>, Refusal> {
        self.list("labels")
    }

    fn list(&self, name: &str) -> Result<Option<Vec<String>>, Refusal> {
        let wrong = || {
            refuse(
                INVALID_PARAMS,
                format!("{name:?} must be a list of strings"),
            )
        };
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(str::to_owned).ok_or_else(wrong))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(wrong()),
        }
    }

    fn flag(&self, name: &str) -> Result<bool, Refusal> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(flag)) => Ok(*flag),
            Some(_) => Err(refuse(
                INVALID_PARAMS,
                format!("\"{name}\" must be true or false"),
            )),
        }
    }

    fn limit(&self, default: usize) -> Result<usize, Refusal> {
        match self.0.get("limit") {
            None | Some(Value::Null) => Ok(default),
            Some(value) => value
                .as_u64()
                .filter(|limit| *limit >= 1)
                .map(|limit| usize::try_from(limit).unwrap_or(MOST).min(MOST))
                .ok_or_else(|| {
                    refuse(
                        INVALID_PARAMS,
                        "\"limit\" must be a whole number, 1 or more",
                    )
                }),
        }
    }

    fn known(&self, names: &[&str]) -> Result<(), Refusal> {
        match self.0.keys().find(|key| !names.contains(&key.as_str())) {
            Some(key) => Err(refuse(
                INVALID_PARAMS,
                format!("unknown argument \"{key}\""),
            )),
            None => Ok(()),
        }
    }
}

/// The text a tool answers with: the JSON of what it did.
fn content(value: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(value).unwrap_or_default() }],
        "isError": false,
    })
}

fn tool_error(message: String) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

impl<B: Backend> Server<B> {
    /// A server over `backend` that says it is version `version` of Leon.
    pub fn new(backend: B, version: impl Into<String>) -> Self {
        Self {
            backend,
            version: version.into(),
            client: None,
        }
    }

    /// Answers one line. `None` when the line asks for no answer: a
    /// notification, a response, or nothing at all.
    pub fn handle(&mut self, line: &str) -> Option<String> {
        if line.trim().is_empty() {
            return None;
        }
        let answer = |id: Value, outcome: Result<Value, Refusal>| {
            let mut message = Map::new();
            message.insert("jsonrpc".into(), "2.0".into());
            message.insert("id".into(), id);
            match outcome {
                Ok(result) => message.insert("result".into(), result),
                Err(refusal) => message.insert(
                    "error".into(),
                    json!({ "code": refusal.code, "message": refusal.message }),
                ),
            };
            Some(Value::Object(message).to_string())
        };
        if line.len() > MAX_LINE {
            return Some(too_long());
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => {
                return answer(
                    Value::Null,
                    Err(refuse(PARSE_ERROR, format!("not JSON: {error}"))),
                )
            }
        };
        let Some(object) = message.as_object() else {
            return answer(
                Value::Null,
                Err(refuse(INVALID_REQUEST, "a message is a JSON object")),
            );
        };
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response to a request this server never made.
            return None;
        };
        let Some(id) = object.get("id").cloned() else {
            // A notification: never answered.
            return None;
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let outcome = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_list() })),
            "tools/call" => self.call(&params),
            other => Err(refuse(
                METHOD_NOT_FOUND,
                format!("the method {other:?} is not supported"),
            )),
        };
        answer(id, outcome)
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let asked = params.get("protocolVersion").and_then(Value::as_str);
        let version = asked
            .and_then(|asked| PROTOCOL_VERSIONS.iter().find(|known| **known == asked))
            .unwrap_or(&PROTOCOL_VERSIONS[0]);
        self.client = params
            .get("clientInfo")
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str)
            .and_then(client_tag);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": self.version },
            "instructions": "The board of tasks of this project. Create a task for work you \
                find and do not do now, move a task to \"doing\" when you start it and to \
                \"done\" when it is finished. Search before creating, to avoid a duplicate.",
        })
    }

    fn call(&mut self, params: &Value) -> Result<Value, Refusal> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| refuse(INVALID_PARAMS, "\"name\" is required"))?;
        if !TOOLS.contains(&name) {
            return Err(refuse(INVALID_PARAMS, format!("there is no tool {name:?}")));
        }
        let empty = Map::new();
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return Err(refuse(INVALID_PARAMS, "\"arguments\" must be an object")),
        };
        let args = Arguments(arguments);
        let result = match name {
            "task_create" => {
                args.known(&[
                    "title", "body", "status", "priority", "labels", "assignee", "parent", "needs",
                    "scope", "project",
                ])?;
                let plan = TaskPatch {
                    parent: args.text("parent")?.map(Some),
                    needs: args.list("needs")?,
                    ..TaskPatch::default()
                };
                let draft = Draft {
                    reach: args.reach()?,
                    title: args.required("title")?,
                    body: args.text("body")?.unwrap_or_default(),
                    status: args.status("status")?,
                    priority: args.priority()?,
                    labels: args.labels()?.unwrap_or_default(),
                    assignee: args.text("assignee")?,
                    agent: self.client.clone(),
                };
                let saved = self.backend.add(draft).and_then(|saved| {
                    if plan.is_empty() {
                        return Ok(saved);
                    }
                    let id = saved.task().id.clone();
                    let planned = self.backend.update(&id, plan).map_err(|error| {
                        format!("the task was made ({id}), but not as a step: {error}")
                    })?;
                    Ok(match saved {
                        SavedTask::New(_) => SavedTask::New(planned),
                        SavedTask::Known(_) => SavedTask::Known(planned),
                    })
                });
                saved.map(|saved| {
                    let created = matches!(saved, SavedTask::New(_));
                    let mut value = json_of(saved.task(), None);
                    if let Some(object) = value.as_object_mut() {
                        object.insert("created".into(), Value::Bool(created));
                        if !created {
                            object.insert(
                                "note".into(),
                                "An open task with this title already exists; nothing was added."
                                    .into(),
                            );
                        }
                    }
                    value
                })
            }
            "task_list" => {
                args.known(&[
                    "status",
                    "include_closed",
                    "assignee",
                    "label",
                    "parent",
                    "scope",
                    "project",
                    "limit",
                ])?;
                let parent = match args.text("parent")? {
                    Some(parent) => match self.backend.get(&parent) {
                        Ok(task) => Some(task.id),
                        Err(message) => return Ok(tool_error(message)),
                    },
                    None => None,
                };
                let filter = TaskFilter {
                    status: args.status("status")?,
                    open_only: !args.flag("include_closed")?,
                    assignee: args.text("assignee")?,
                    label: args.text("label")?,
                    parent,
                };
                self.backend
                    .list(args.reach()?, filter, args.limit(DEFAULT_LISTED)?)
                    .map(|tasks| {
                        Value::Array(tasks.iter().map(|task| json_of(task, None)).collect())
                    })
            }
            "task_get" => {
                args.known(&["id"])?;
                self.backend
                    .get(&args.required("id")?)
                    .map(|task| json_of(&task, None))
            }
            "task_update" => {
                args.known(&[
                    "id", "title", "body", "status", "priority", "labels", "assignee", "branch",
                    "parent", "needs",
                ])?;
                let patch = TaskPatch {
                    title: args.text("title")?,
                    body: args.text("body")?,
                    status: args.status("status")?,
                    priority: args.priority()?,
                    labels: args.labels()?,
                    assignee: args
                        .text("assignee")?
                        .map(|assignee| Some(assignee).filter(|a| !a.trim().is_empty())),
                    branch: args
                        .text("branch")?
                        .map(|branch| Some(branch).filter(|b| !b.trim().is_empty())),
                    parent: args
                        .text("parent")?
                        .map(|parent| Some(parent).filter(|p| !p.trim().is_empty())),
                    needs: args.list("needs")?,
                };
                self.backend
                    .update(&args.required("id")?, patch)
                    .map(|task| json_of(&task, None))
            }
            "task_move" => {
                args.known(&["id", "status", "before"])?;
                let status = args
                    .status("status")?
                    .ok_or_else(|| refuse(INVALID_PARAMS, "\"status\" is required"))?;
                let before = args.text("before")?;
                self.backend
                    .move_to(&args.required("id")?, status, before.as_deref())
                    .map(|task| json_of(&task, None))
            }
            _ => {
                args.known(&["query", "scope", "project", "limit"])?;
                self.backend
                    .search(&args.required("query")?, args.reach()?, args.limit(10)?)
                    .map(|hits| {
                        Value::Array(
                            hits.iter()
                                .map(|hit| json_of(&hit.task, Some(&hit.snippet)))
                                .collect(),
                        )
                    })
            }
        };
        Ok(match result {
            Ok(value) => content(&value),
            Err(message) => tool_error(message),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::Service;
    use leon_core::Store;

    fn server() -> Server<Service> {
        let service =
            Service::new(Store::open_in_memory().unwrap(), "/srv/api".to_owned()).at(|| 1_000);
        Server::new(service, "0.0.0-test")
    }

    fn ask(server: &mut Server<Service>, message: Value) -> Value {
        let line = server.handle(&message.to_string()).expect("an answer");
        serde_json::from_str(&line).unwrap()
    }

    fn call(server: &mut Server<Service>, tool: &str, arguments: Value) -> Value {
        ask(
            server,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": arguments },
            }),
        )
    }

    /// The JSON a tool answered with, and whether it was an error.
    fn payload(answer: &Value) -> (Value, bool) {
        let result = &answer["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        let value = serde_json::from_str(text).unwrap_or(Value::String(text.to_owned()));
        (value, result["isError"].as_bool().unwrap())
    }

    #[test]
    fn king_names_the_project_of_each_task_and_sees_every_board() {
        // King's server runs in no project, and sees every board.
        let service = Service::new(Store::open_in_memory().unwrap(), "/data/king".to_owned())
            .at(|| 1_000)
            .anywhere(true);
        let mut server = Server::new(service, "0.0.0-test");
        ask(
            &mut server,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "clientInfo": { "name": "Claude Code" } },
            }),
        );
        let (created, failed) = payload(&call(
            &mut server,
            "task_create",
            json!({ "title": "Fix the login", "project": "/srv/web" }),
        ));
        assert!(!failed, "{created}");
        assert_eq!(created["scope"], "project");
        assert_eq!(created["project"], "/srv/web");
        let id = created["id"].as_str().unwrap().to_owned();
        let (found, failed) = payload(&call(&mut server, "task_get", json!({ "id": id })));
        assert!(!failed, "a task of another project is King's too: {found}");
        let (listed, _) = payload(&call(
            &mut server,
            "task_list",
            json!({ "project": "/srv/web" }),
        ));
        assert_eq!(listed.as_array().map(Vec::len), Some(1));
        let both = call(
            &mut server,
            "task_create",
            json!({ "title": "x", "project": "/srv/web", "scope": "global" }),
        );
        assert_eq!(
            both["error"]["code"], INVALID_PARAMS,
            "a project and the global board at once is refused"
        );
    }

    #[test]
    fn the_server_introduces_itself_and_lists_its_tools() {
        let mut server = server();
        let hello = ask(
            &mut server,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-03-26", "clientInfo": { "name": "claude-code" } },
            }),
        );
        assert_eq!(hello["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(hello["result"]["serverInfo"]["name"], SERVER_NAME);
        let listed = ask(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        );
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOLS);
        assert!(!names.iter().any(|name| name.contains("delete")));
        // A notification is not answered.
        assert_eq!(
            server.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            None
        );
    }

    #[test]
    fn a_task_is_made_a_step_of_another_with_what_it_needs() {
        let mut server = server();
        let id = |value: &Value| value["id"].as_str().unwrap().to_owned();
        let (goal, _) = payload(&call(
            &mut server,
            "task_create",
            json!({ "title": "Ship billing" }),
        ));
        let (schema, _) = payload(&call(
            &mut server,
            "task_create",
            json!({ "title": "Billing schema", "parent": id(&goal) }),
        ));
        assert_eq!(schema["parent"], goal["id"]);
        let (api, failed) = payload(&call(
            &mut server,
            "task_create",
            json!({ "title": "Billing API", "parent": id(&goal), "needs": [id(&schema)] }),
        ));
        assert!(!failed, "{api}");
        assert_eq!(api["needs"], json!([id(&schema)]));
        let (steps, _) = payload(&call(
            &mut server,
            "task_list",
            json!({ "parent": goal["short_id"] }),
        ));
        assert_eq!(steps.as_array().map(Vec::len), Some(2));
        // Needs never go round.
        let (refused, failed) = payload(&call(
            &mut server,
            "task_update",
            json!({ "id": id(&schema), "needs": [id(&api)] }),
        ));
        assert!(failed, "{refused}");
    }

    #[test]
    fn an_agent_creates_moves_and_finds_a_task() {
        let mut server = server();
        ask(
            &mut server,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "clientInfo": { "name": "Claude Code" } },
            }),
        );
        let (created, error) = payload(&call(
            &mut server,
            "task_create",
            json!({ "title": "Cover the retry path", "labels": ["tests"], "priority": "high" }),
        ));
        assert!(!error);
        assert_eq!(created["created"], true);
        assert_eq!(created["status"], "todo");
        assert_eq!(created["agent"], "claude");
        let id = created["short_id"].as_str().unwrap().to_owned();

        // The same title again is the same task.
        let (again, _) = payload(&call(
            &mut server,
            "task_create",
            json!({ "title": "cover the retry path" }),
        ));
        assert_eq!(again["created"], false);
        assert_eq!(again["id"], created["id"]);

        let (moved, error) = payload(&call(
            &mut server,
            "task_move",
            json!({ "id": id, "status": "in progress" }),
        ));
        assert!(!error);
        assert_eq!(moved["status"], "doing");

        let (listed, _) = payload(&call(&mut server, "task_list", json!({})));
        assert_eq!(listed.as_array().unwrap().len(), 1);
        let (found, _) = payload(&call(
            &mut server,
            "task_search",
            json!({ "query": "retry" }),
        ));
        assert_eq!(found[0]["title"], "Cover the retry path");

        let (closed, _) = payload(&call(
            &mut server,
            "task_update",
            json!({ "id": id, "status": "done", "assignee": "" }),
        ));
        assert_eq!(closed["status"], "done");
        assert_eq!(closed["closed_at"], 1_000);
        // Closed tasks are left out of a listing unless asked for.
        let (open, _) = payload(&call(&mut server, "task_list", json!({})));
        assert!(open.as_array().unwrap().is_empty());
        let (all, _) = payload(&call(
            &mut server,
            "task_list",
            json!({ "include_closed": true }),
        ));
        assert_eq!(all.as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_refused_call_says_why_and_a_malformed_one_is_a_protocol_error() {
        let mut server = server();
        let (message, error) = payload(&call(&mut server, "task_get", json!({ "id": "abcdef12" })));
        assert!(error);
        assert!(message.as_str().unwrap().contains("no task"));

        let (message, error) = payload(&call(
            &mut server,
            "task_create",
            json!({ "title": "x".repeat(500) }),
        ));
        assert!(error);
        assert!(message.as_str().unwrap().contains("title has"));

        let wrong = call(
            &mut server,
            "task_create",
            json!({ "title": "x", "color": "red" }),
        );
        assert_eq!(wrong["error"]["code"], INVALID_PARAMS);
        let wrong = call(
            &mut server,
            "task_create",
            json!({ "title": "x", "status": "later" }),
        );
        assert_eq!(wrong["error"]["code"], INVALID_PARAMS);
        let wrong = call(&mut server, "task_delete", json!({ "id": "abcdef12" }));
        assert_eq!(wrong["error"]["code"], INVALID_PARAMS);
        let not_json = server.handle("{oops").unwrap();
        assert!(not_json.contains("-32700"));
    }
}
