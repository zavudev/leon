//! One connection to King's service, and starting the service when nothing
//! answers.
//!
//! The socket's folder must be the user's alone and the process at the other
//! end must be the user's (the keeper's checks, `leon_link::local`): only
//! then is anything written. A client that finds no service starts one,
//! detached, the way the window starts the keeper, and waits for it a few
//! seconds.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use leon_king::coordinator::Target;
use leon_king::protocol::{Entries, Envelope, Notice, Reply, Request, Status};
use leon_king::transcript::Kind;
use serde_json::Value;

use super::Paths;

/// How long a started service has to answer.
const START_WITHIN: Duration = Duration::from_secs(10);

/// A connection to the service.
pub struct Client {
    reader: Reader,
    writer: Writer,
}

/// The half of a connection that writes requests.
pub struct Writer {
    stream: UnixStream,
    next: u64,
}

/// The half of a connection that reads what the service sends.
pub struct Reader {
    stream: BufReader<UnixStream>,
}

/// What the service sent that is not a reply.
pub enum Incoming {
    /// The reply to a request.
    Reply(Reply),
    /// A notice of a subscription.
    Notice(Notice),
}

impl Client {
    /// Dials the service, after checking whose it is.
    pub fn dial(paths: &Paths) -> Result<Self, String> {
        let ours = leon_link::local::our_uid();
        let dir = paths.socket.parent().ok_or("the socket has no folder")?;
        leon_link::local::check_private_dir(dir, ours).map_err(|why| why.to_string())?;
        let stream = UnixStream::connect(&paths.socket).map_err(|error| error.to_string())?;
        if !leon_link::local::peer_allowed(leon_link::local::peer_uid(&stream), ours) {
            return Err("the socket belongs to another user".to_owned());
        }
        let writer = stream.try_clone().map_err(|error| error.to_string())?;
        Ok(Self {
            reader: Reader {
                stream: BufReader::new(stream),
            },
            writer: Writer {
                stream: writer,
                next: 1,
            },
        })
    }

    /// Dials the service, starting it first when nothing answers.
    pub fn dial_or_start(paths: &Paths) -> Result<Self, String> {
        if let Ok(client) = Self::dial(paths) {
            return Ok(client);
        }
        start_service(paths)?;
        let deadline = Instant::now() + START_WITHIN;
        loop {
            match Self::dial(paths) {
                Ok(client) => return Ok(client),
                Err(error) if Instant::now() >= deadline => {
                    return Err(format!("King's service did not start: {error}"))
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    /// Sends a request and returns its id.
    pub fn send(&mut self, request: Request) -> Result<u64, String> {
        self.writer.send(request)
    }

    /// The next thing the service sends.
    pub fn read(&mut self) -> Result<Incoming, String> {
        self.reader.read()
    }

    /// The two halves, for a reader on a thread of its own.
    pub fn split(self) -> (Writer, Reader) {
        (self.writer, self.reader)
    }

    /// Asks and waits for the answer, skipping notices.
    pub fn ask(&mut self, request: Request) -> Result<Value, String> {
        let id = self.send(request)?;
        loop {
            if let Incoming::Reply(reply) = self.read()? {
                if reply.id != id {
                    continue;
                }
                return match reply.error {
                    Some(error) if !reply.ok => Err(error),
                    _ => Ok(reply.result),
                };
            }
        }
    }
}

impl Writer {
    /// Sends a request and returns its id.
    pub fn send(&mut self, request: Request) -> Result<u64, String> {
        let id = self.next;
        self.next += 1;
        self.send_as(id, request)?;
        Ok(id)
    }

    /// Sends a request under an id the caller chose.
    pub fn send_as(&mut self, id: u64, request: Request) -> Result<(), String> {
        let line = serde_json::to_string(&Envelope { id, request }).map_err(|e| e.to_string())?;
        self.stream
            .write_all(format!("{line}\n").as_bytes())
            .map_err(|error| error.to_string())
    }
}

impl Reader {
    /// The next thing the service sends.
    pub fn read(&mut self) -> Result<Incoming, String> {
        let mut line = String::new();
        let read = self
            .stream
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("King's service closed the connection".to_owned());
        }
        let value: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if value.get("notice").is_some() {
            let notice = serde_json::from_value(value).map_err(|error| error.to_string())?;
            return Ok(Incoming::Notice(notice));
        }
        let reply = serde_json::from_value(value).map_err(|error| error.to_string())?;
        Ok(Incoming::Reply(reply))
    }
}

/// Starts `leon king serve` detached from this process.
fn start_service(paths: &Paths) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = leon_remote::spawn::std_child(exe.to_string_lossy().into_owned());
    command
        .arg("--data-dir")
        .arg(&paths.data_dir)
        .args(["king", "serve"]);
    crate::durable::detach(&mut command);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Runs a request for `leon king …` and prints what it gives. After `say`,
/// waits for King to finish and prints its answer.
pub fn ask_and_print(paths: &Paths, request: Request, wait: bool) -> Result<(), String> {
    let mut client = Client::dial_or_start(paths)?;
    if !wait {
        let result = client.ask(request.clone())?;
        match request {
            Request::Status => {
                print_status(&serde_json::from_value(result).map_err(|e| e.to_string())?)
            }
            _ if result.is_null() => println!("Done."),
            _ => println!(
                "{}",
                serde_json::to_string_pretty(&result).unwrap_or_default()
            ),
        }
        return Ok(());
    }
    let before: Status =
        serde_json::from_value(client.ask(Request::Status)?).map_err(|e| e.to_string())?;
    client.ask(Request::Subscribe)?;
    client.ask(request)?;
    let mut seen = before.brain.last_seq;
    let mut started = false;
    loop {
        let Incoming::Notice(notice) = client.read()? else {
            continue;
        };
        let Notice::Changed { status } = notice else {
            continue;
        };
        started |= status.brain.busy;
        // What King said so far, as it comes.
        if status.brain.last_seq != seen {
            let result = client.ask(Request::Transcript {
                target: Target::Brain,
                since: seen,
                limit: Some(200),
            })?;
            let entries: Entries = serde_json::from_value(result).map_err(|e| e.to_string())?;
            for entry in &entries {
                print_entry(entry);
            }
            if let Some(last) = entries.last() {
                seen = Some(last.seq);
            }
        }
        if let Some(open) = status.brain.questions.first() {
            println!(
                "\nKing waits on question {}. Answer it from the window, or tell it what to do \
                 with `leon king say`.",
                open.id
            );
            return Ok(());
        }
        if started && !status.brain.busy && status.brain.queued == 0 {
            return Ok(());
        }
    }
}

/// Prints what King or a worker said lately.
pub fn print_log(paths: &Paths, target: Target) -> Result<(), String> {
    let mut client = Client::dial_or_start(paths)?;
    let result = client.ask(Request::Transcript {
        target,
        since: None,
        limit: Some(60),
    })?;
    let entries: Entries = serde_json::from_value(result).map_err(|e| e.to_string())?;
    for entry in &entries {
        print_entry(entry);
    }
    Ok(())
}

fn print_entry(entry: &leon_king::transcript::Entry) {
    match entry.kind {
        Kind::Prompt => println!("\n> {}\n", entry.text.replace('\n', "\n> ")),
        Kind::News => println!("  ! {}", entry.text.replace('\n', "\n  ! ")),
        Kind::Agent => println!("{}\n", entry.text),
        Kind::Tool => println!("  · {}", entry.text),
        Kind::Question => println!("  ? {}", entry.text),
        Kind::Notice => println!("  ! {}", entry.text),
        Kind::Report => println!("  ── {}\n", entry.text.replace('\n', "\n  ── ")),
    }
}

fn print_status(status: &Status) {
    let brain = &status.brain;
    println!(
        "King: {}{}{}, {}",
        brain.agent,
        brain
            .model
            .as_deref()
            .map(|m| format!(" {m}"))
            .unwrap_or_default(),
        brain
            .effort
            .as_deref()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default(),
        if brain.busy { "working" } else { "waiting" }
    );
    match status.daily_budget_usd {
        Some(budget) => println!(
            "Spent today: ${:.2} of ${budget:.2}",
            status.spent_today_usd
        ),
        None => println!("Spent today: ${:.2}", status.spent_today_usd),
    }
    if status.workers.is_empty() {
        println!("No workers.");
    }
    for worker in &status.workers {
        println!(
            "#{} {:<9} {} — {}{} in {}",
            worker.id,
            format!("{:?}", worker.state).to_lowercase(),
            worker.title,
            worker.conversation.agent,
            worker
                .conversation
                .model
                .as_deref()
                .map(|m| format!(" {m}"))
                .unwrap_or_default(),
            worker.cwd
        );
    }
}
