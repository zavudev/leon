//! `leon king mcp`: King's tools, an MCP server over standard streams.
//!
//! The agent King runs on starts this server (the service names it in the
//! turn, see `serve.rs`) and talks to it one JSON-RPC message per line, as
//! with `leon memory mcp` and `leon task mcp`. Most tools are a request to
//! King's service ([`Backend::ask`]); the ones about a project's git state
//! and pull requests run `git` and `gh` here ([`Backend::run`]), with the
//! guards that do not depend on what King was told: nothing is pushed to a
//! project's default branch, nothing is forced, nothing is merged.
//!
//! [`Server::handle`] is the conversation without the pipes, so every
//! exchange is tested with a backend that records what it was asked.

use leon_king::coordinator::{Job, Reply as Answer, Target};
use leon_king::protocol::{Request, WorkerRequest};
use serde_json::{json, Map, Value};

use super::client::Client;
use super::Paths;
use crate::task_mcp::PROTOCOL_VERSIONS;

/// The name the server gives for itself.
pub const SERVER_NAME: &str = "leon-king";

/// The names of the tools, in the order they are listed.
pub const TOOLS: [&str; 13] = [
    "king_status",
    "projects_list",
    "project_status",
    "usage_status",
    "worker_start",
    "worker_send",
    "worker_answer",
    "worker_interrupt",
    "worker_stop",
    "worker_transcript",
    "worktree_diff",
    "pr_open",
    "notify_user",
];

/// How much of a diff is given back, in characters.
const DIFF_CHARS: usize = 60_000;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What the server needs done.
pub trait Backend {
    /// A request to King's service.
    fn ask(&mut self, request: Request) -> Result<Value, String>;
    /// Runs `program` with `args` in `cwd` and gives what it printed, or
    /// what it said on failing.
    fn run(&mut self, program: &str, args: &[&str], cwd: &str) -> Result<String, String>;
}

/// The real backend: the service's socket, and this computer's processes.
pub struct Live {
    paths: Paths,
    client: Option<Client>,
}

impl Backend for Live {
    fn ask(&mut self, request: Request) -> Result<Value, String> {
        if self.client.is_none() {
            self.client = Some(Client::dial_or_start(&self.paths)?);
        }
        let client = self.client.as_mut().ok_or("no connection")?;
        let answer = client.ask(request);
        if answer
            .as_ref()
            .is_err_and(|error| error.contains("closed the connection"))
        {
            self.client = None;
        }
        answer
    }

    fn run(&mut self, program: &str, args: &[&str], cwd: &str) -> Result<String, String> {
        let output = leon_remote::spawn::std_child(program)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("{program}: {error}"))?;
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim_end().to_owned();
        if output.status.success() {
            Ok(text(&output.stdout))
        } else {
            let said = text(&output.stderr);
            Err(if said.is_empty() {
                text(&output.stdout)
            } else {
                said
            })
        }
    }
}

/// Runs the server on standard input and output until its input ends.
pub fn run(paths: &Paths) -> Result<(), String> {
    let mut server = Server::new(Live {
        paths: paths.clone(),
        client: None,
    });
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout().lock();
    converse(&mut server, &mut input, &mut output).map_err(|error| error.to_string())
}

/// The MCP conversation over two pipes: a line in, at most a line out.
pub fn converse<B: Backend>(
    server: &mut Server<B>,
    input: &mut impl std::io::BufRead,
    output: &mut impl std::io::Write,
) -> std::io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if let Some(answer) = server.handle(&line) {
            output.write_all(answer.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

/// The server.
pub struct Server<B> {
    backend: B,
}

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
    let worker = json!({ "type": "integer", "minimum": 1, "description": "The worker's number." });
    json!([
        tool(
            "king_status",
            "Your own conversation and every worker that is still yours: what each is doing, \
             on which agent and model, what it last said, and what was spent today.",
            json!({}),
            &[],
        ),
        tool(
            "projects_list",
            "Every project Leon knows, on every machine, with its root folder and worktrees.",
            json!({}),
            &[],
        ),
        tool(
            "project_status",
            "A project's git state now: the current branch, changes not committed, its \
             worktrees, and its open pull requests when GitHub's `gh` is set up.",
            json!({ "project": { "type": "string", "description": "The project's name." } }),
            &["project"],
        ),
        tool(
            "usage_status",
            "Where each agent stands: installed or not, how much of its usage window it has \
             used, what was spent today against the budget, and the tiers of workers.",
            json!({}),
            &[],
        ),
        tool(
            "worker_start",
            "Start a worker: another agent that works on its own, in a new worktree of the \
             project (or in `cwd`, an existing worktree). Pick the cheapest tier that will do \
             the job well, or name an agent and model. You are woken when it finishes, fails \
             or asks something; do not poll.",
            json!({
                "title": { "type": "string", "description": "What it is for, in a few words." },
                "prompt": { "type": "string", "description": "The whole task: what, why, how to know it is done." },
                "project": { "type": "string" },
                "branch": { "type": "string", "description": "The new worktree's branch; made up when absent." },
                "cwd": { "type": "string", "description": "An existing worktree to work in instead." },
                "tier": { "type": "string", "description": "cheap, standard, premium, or one king.toml names." },
                "agent": { "type": "string", "enum": ["claude", "codex", "opencode", "grok", "cursor"] },
                "model": { "type": "string" },
                "effort": { "type": "string" },
                "mode": { "type": "string", "description": "The agent's permission mode." },
                "job": { "type": "string", "enum": ["code", "research"], "default": "code" },
            }),
            &["title", "prompt"],
        ),
        tool(
            "worker_send",
            "Tell a worker more. It hears it now if it waits, or when its turn ends.",
            json!({ "worker": worker.clone(), "text": { "type": "string" } }),
            &["worker", "text"],
        ),
        tool(
            "worker_answer",
            "Answer a question a worker waits on. A permission takes `choice` (allow, always, \
             deny, or a choice's id) and may take `text` as the reason; options take `choice` \
             (a label) or `text`; a plan takes `approve` and may take `text`.",
            json!({
                "worker": worker.clone(),
                "question": { "type": "string" },
                "choice": { "type": "string" },
                "approve": { "type": "boolean" },
                "text": { "type": "string" },
            }),
            &["worker", "question"],
        ),
        tool(
            "worker_interrupt",
            "Stop what a worker is doing now and keep its conversation.",
            json!({ "worker": worker.clone() }),
            &["worker"],
        ),
        tool(
            "worker_stop",
            "Stop a worker for good: its turn ends and what waited for it is dropped.",
            json!({ "worker": worker.clone() }),
            &["worker"],
        ),
        tool(
            "worker_transcript",
            "What a worker said: its prompts, answers, tools and questions, newest last.",
            json!({
                "worker": worker.clone(),
                "since": { "type": "integer", "description": "Only entries after this number." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 40 },
            }),
            &["worker"],
        ),
        tool(
            "worktree_diff",
            "What a worker's branch changed against the project's default branch: the commits \
             and the diff.",
            json!({ "worker": worker.clone() }),
            &["worker"],
        ),
        tool(
            "pr_open",
            "Push a worker's branch and open a pull request for it. Never the default branch, \
             never forced; merging is the person's.",
            json!({
                "worker": worker.clone(),
                "title": { "type": "string" },
                "body": { "type": "string" },
                "draft": { "type": "boolean", "default": false },
            }),
            &["worker", "title"],
        ),
        tool(
            "notify_user",
            "Tell the person, on their desktop: something they asked for is done, or needs \
             their decision.",
            json!({ "title": { "type": "string" }, "body": { "type": "string" } }),
            &["title"],
        ),
    ])
}

impl<B: Backend> Server<B> {
    /// A server over `backend`.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// One message in, at most one out.
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
        let method = object.get("method").and_then(Value::as_str)?;
        let id = object.get("id").cloned()?;
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let outcome = match method {
            "initialize" => Ok(initialize(&params)),
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
        // A tool that ran and refused is a result the agent reads.
        Ok(match self.tool(name, arguments) {
            Ok(value) => {
                let text = match value {
                    Value::String(text) => text,
                    Value::Null => "Done.".to_owned(),
                    other => serde_json::to_string_pretty(&other).unwrap_or_default(),
                };
                json!({ "content": [{ "type": "text", "text": text }], "isError": false })
            }
            Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
        })
    }

    fn tool(&mut self, name: &str, arguments: &Map<String, Value>) -> Result<Value, String> {
        let text = |key: &str| -> Option<String> {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let needed = |key: &str| text(key).ok_or_else(|| format!("\"{key}\" is required"));
        let worker = || -> Result<u32, String> {
            arguments
                .get("worker")
                .and_then(Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(|| "\"worker\" is required: the worker's number".to_owned())
        };
        match name {
            "king_status" => self.backend.ask(Request::Status),
            "projects_list" => self.backend.ask(Request::Projects),
            "usage_status" => self.backend.ask(Request::Usage),
            "project_status" => self.project_status(&needed("project")?),
            "worker_start" => {
                let job = match text("job").as_deref() {
                    None | Some("code") => Job::Code,
                    Some("research") => Job::Research,
                    Some(other) => return Err(format!("a job is code or research, not {other:?}")),
                };
                self.backend
                    .ask(Request::StartWorker(Box::new(WorkerRequest {
                        title: needed("title")?,
                        prompt: needed("prompt")?,
                        project: text("project"),
                        branch: text("branch"),
                        cwd: text("cwd"),
                        tier: text("tier"),
                        agent: text("agent"),
                        model: text("model"),
                        effort: text("effort"),
                        mode: text("mode"),
                        job,
                    })))
            }
            "worker_send" => self.backend.ask(Request::TellWorker {
                worker: worker()?,
                text: needed("text")?,
            }),
            "worker_answer" => self.backend.ask(Request::Answer {
                target: Target::Worker(worker()?),
                question: needed("question")?,
                reply: Answer {
                    choice: text("choice"),
                    approve: arguments.get("approve").and_then(Value::as_bool),
                    text: text("text"),
                },
            }),
            "worker_interrupt" => self.backend.ask(Request::Interrupt {
                target: Target::Worker(worker()?),
            }),
            "worker_stop" => self.backend.ask(Request::StopWorker { worker: worker()? }),
            "worker_transcript" => self.backend.ask(Request::Transcript {
                target: Target::Worker(worker()?),
                since: arguments.get("since").and_then(Value::as_u64),
                limit: Some(
                    arguments
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map_or(40, |limit| limit.clamp(1, 200) as usize),
                ),
            }),
            "worktree_diff" => self.worktree_diff(worker()?),
            "pr_open" => self.pr_open(
                worker()?,
                &needed("title")?,
                &text("body").unwrap_or_default(),
                arguments.get("draft").and_then(Value::as_bool) == Some(true),
            ),
            "notify_user" => self.backend.ask(Request::Notify {
                title: needed("title")?,
                body: text("body").unwrap_or_default(),
            }),
            other => Err(format!("there is no tool {other:?}")),
        }
    }

    /// The folder of a worker that is still King's.
    fn worker_folder(&mut self, worker: u32) -> Result<String, String> {
        let status = self.backend.ask(Request::Status)?;
        status["workers"]
            .as_array()
            .and_then(|workers| {
                workers
                    .iter()
                    .find(|w| w["id"].as_u64() == Some(u64::from(worker)))
            })
            .and_then(|w| w["cwd"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("there is no worker #{worker} of yours"))
    }

    fn project_status(&mut self, name: &str) -> Result<Value, String> {
        let projects = self.backend.ask(Request::Projects)?;
        let project = projects
            .as_array()
            .and_then(|all| all.iter().find(|p| p["name"].as_str() == Some(name)))
            .ok_or_else(|| format!("Leon knows no project {name:?}"))?
            .clone();
        if project["local"].as_bool() != Some(true) {
            return Ok(json!({
                "project": project,
                "note": "it is on another machine; its git state is not read from here yet",
            }));
        }
        let root = project["root"].as_str().unwrap_or_default().to_owned();
        let status = self
            .backend
            .run("git", &["status", "--short", "--branch"], &root)?;
        let worktrees = self
            .backend
            .run("git", &["worktree", "list"], &root)
            .unwrap_or_default();
        let log = self
            .backend
            .run("git", &["log", "--oneline", "-10"], &root)
            .unwrap_or_default();
        let prs = self
            .backend
            .run(
                "gh",
                &[
                    "pr",
                    "list",
                    "--limit",
                    "20",
                    "--json",
                    "number,title,headRefName,isDraft,reviewDecision,statusCheckRollup",
                ],
                &root,
            )
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        Ok(json!({
            "project": name,
            "root": root,
            "status": status,
            "worktrees": worktrees,
            "recent_commits": log,
            "pull_requests": prs.unwrap_or(Value::Null),
        }))
    }

    fn base_of(&mut self, cwd: &str) -> String {
        self.backend
            .run(
                "git",
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
                cwd,
            )
            .map(|name| name.trim().to_owned())
            .unwrap_or_else(|_| "main".to_owned())
    }

    fn worktree_diff(&mut self, worker: u32) -> Result<Value, String> {
        let cwd = self.worker_folder(worker)?;
        let base = self.base_of(&cwd);
        let range = format!("{base}...HEAD");
        let commits = self
            .backend
            .run("git", &["log", "--oneline", &range], &cwd)?;
        let uncommitted = self.backend.run("git", &["status", "--short"], &cwd)?;
        let diff = self.backend.run("git", &["diff", &range], &cwd)?;
        Ok(json!({
            "base": base,
            "commits": commits,
            "uncommitted": uncommitted,
            "diff": leon_king::transcript::cut(&diff, DIFF_CHARS),
        }))
    }

    fn pr_open(
        &mut self,
        worker: u32,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<Value, String> {
        let cwd = self.worker_folder(worker)?;
        let branch = self
            .backend
            .run("git", &["symbolic-ref", "--short", "HEAD"], &cwd)
            .map_err(|_| "the worktree is not on a branch".to_owned())?;
        let base = self.base_of(&cwd);
        let default = base.trim_start_matches("origin/");
        if branch == default || ["main", "master"].contains(&branch.as_str()) {
            return Err(format!(
                "{branch} is the default branch: nothing is pushed to it from here"
            ));
        }
        let uncommitted = self.backend.run("git", &["status", "--porcelain"], &cwd)?;
        if !uncommitted.trim().is_empty() {
            return Err(format!(
                "the worktree has changes not committed; have the worker commit them first:\n{uncommitted}"
            ));
        }
        self.backend
            .run("git", &["push", "--set-upstream", "origin", &branch], &cwd)?;
        let mut args = vec![
            "pr", "create", "--head", &branch, "--base", default, "--title", title, "--body", body,
        ];
        if draft {
            args.push("--draft");
        }
        let url = self.backend.run("gh", &args, &cwd)?;
        Ok(json!({ "branch": branch, "base": default, "pull_request": url }))
    }
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .and_then(|asked| PROTOCOL_VERSIONS.iter().find(|known| **known == asked))
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        "instructions": "King Leon's tools: the projects Leon knows, the agents' usage, and \
            the workers you start, steer and stop. You are woken with news of your workers; \
            never poll them.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that records what it was asked and answers from a script.
    #[derive(Default)]
    struct Fake {
        asked: Vec<Request>,
        ran: Vec<(String, Vec<String>, String)>,
        branch: String,
        dirty: bool,
    }

    impl Backend for Fake {
        fn ask(&mut self, request: Request) -> Result<Value, String> {
            self.asked.push(request.clone());
            Ok(match request {
                Request::Status => json!({ "workers": [{ "id": 2, "cwd": "/w/fix" }] }),
                Request::Projects => {
                    json!([{ "name": "leon", "root": "/code/leon", "local": true }])
                }
                _ => Value::Null,
            })
        }

        fn run(&mut self, program: &str, args: &[&str], cwd: &str) -> Result<String, String> {
            self.ran.push((
                program.to_owned(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
                cwd.to_owned(),
            ));
            Ok(match args {
                ["symbolic-ref", "--short", "HEAD"] => self.branch.clone(),
                ["symbolic-ref", ..] => "origin/main".to_owned(),
                ["status", "--porcelain"] if self.dirty => " M src/lib.rs".to_owned(),
                ["pr", "create", ..] => "https://github.com/z/leon/pull/9".to_owned(),
                _ => String::new(),
            })
        }
    }

    fn call(server: &mut Server<Fake>, name: &str, arguments: Value) -> (String, bool) {
        let line = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        });
        let answer: Value =
            serde_json::from_str(&server.handle(&line.to_string()).unwrap()).unwrap();
        let result = &answer["result"];
        (
            result["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            result["isError"].as_bool().unwrap_or(true),
        )
    }

    #[test]
    fn every_tool_is_listed_with_a_schema() {
        let tools = tool_list();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOLS);
        let mut server = Server::new(Fake::default());
        let hello = server
            .handle(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#)
            .unwrap();
        assert!(hello.contains("leon-king"));
        assert!(server
            .handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
    }

    #[test]
    fn a_worker_is_asked_for_with_what_king_said_and_nothing_else() {
        let mut server = Server::new(Fake::default());
        let (_, error) = call(
            &mut server,
            "worker_start",
            json!({ "title": "fix login", "prompt": "fix it", "project": "leon", "tier": "cheap", "job": "research" }),
        );
        assert!(!error);
        assert_eq!(
            server.backend.asked,
            [Request::StartWorker(Box::new(WorkerRequest {
                title: "fix login".into(),
                prompt: "fix it".into(),
                project: Some("leon".into()),
                tier: Some("cheap".into()),
                job: Job::Research,
                ..WorkerRequest::default()
            }))]
        );
        let (why, error) = call(&mut server, "worker_start", json!({ "title": "x" }));
        assert!(error);
        assert!(why.contains("prompt"));
        let (why, error) = call(&mut server, "worker_send", json!({ "text": "more" }));
        assert!(error && why.contains("worker"));
    }

    #[test]
    fn an_answer_reaches_the_workers_question() {
        let mut server = Server::new(Fake::default());
        call(
            &mut server,
            "worker_answer",
            json!({ "worker": 3, "question": "q1", "choice": "deny", "text": "not yet" }),
        );
        assert_eq!(
            server.backend.asked,
            [Request::Answer {
                target: Target::Worker(3),
                question: "q1".into(),
                reply: Answer {
                    choice: Some("deny".into()),
                    approve: None,
                    text: Some("not yet".into()),
                },
            }]
        );
    }

    #[test]
    fn a_pull_request_is_pushed_and_opened_from_the_workers_branch_only() {
        let mut server = Server::new(Fake {
            branch: "king/fix-login".into(),
            ..Fake::default()
        });
        let (text, error) = call(
            &mut server,
            "pr_open",
            json!({ "worker": 2, "title": "fix: login" }),
        );
        assert!(!error, "{text}");
        assert!(text.contains("pull/9"));
        let pushed = server
            .backend
            .ran
            .iter()
            .find(|(_, args, _)| args.first().map(String::as_str) == Some("push"))
            .unwrap();
        assert_eq!(
            pushed.1,
            ["push", "--set-upstream", "origin", "king/fix-login"]
        );
        assert_eq!(pushed.2, "/w/fix");
        assert!(!server.backend.ran.iter().any(|(_, args, _)| args
            .iter()
            .any(|arg| arg.contains("force") || arg == "merge")));

        let mut on_main = Server::new(Fake {
            branch: "main".into(),
            ..Fake::default()
        });
        let (why, error) = call(
            &mut on_main,
            "pr_open",
            json!({ "worker": 2, "title": "x" }),
        );
        assert!(error && why.contains("default branch"), "{why}");
        assert!(!on_main
            .backend
            .ran
            .iter()
            .any(|(_, args, _)| args[0] == "push"));

        let mut dirty = Server::new(Fake {
            branch: "king/x".into(),
            dirty: true,
            ..Fake::default()
        });
        let (why, error) = call(&mut dirty, "pr_open", json!({ "worker": 2, "title": "x" }));
        assert!(error && why.contains("not committed"), "{why}");

        let (why, error) = call(&mut server, "pr_open", json!({ "worker": 7, "title": "x" }));
        assert!(error && why.contains("no worker #7"), "{why}");
    }

    #[test]
    fn a_projects_state_is_read_in_its_root() {
        let mut server = Server::new(Fake::default());
        let (text, error) = call(&mut server, "project_status", json!({ "project": "leon" }));
        assert!(!error, "{text}");
        assert!(server
            .backend
            .ran
            .iter()
            .all(|(_, _, cwd)| cwd == "/code/leon"));
        let (why, error) = call(&mut server, "project_status", json!({ "project": "nope" }));
        assert!(error && why.contains("no project"));
    }
}
