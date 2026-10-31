//! King Leon's line through the host: what lets a paired device speak to
//! King on this computer.
//!
//! The host reads none of it. A session's first [`Message::King`] opens a
//! connection to King's service through the [`KingDoor`] this installation
//! gave the host; every line after it is written to that connection, and
//! every line King writes back goes to the device as a [`Message::King`] of
//! its own: replies and notices alike. When the connection cannot be opened
//! or closes, the device hears [`Message::KingClosed`], and its next line
//! opens a new one. The connection ends with the session.
//!
//! The door is the application's (`leon king`'s socket, with its checks of
//! whose it is); the host only knows that a line goes in and lines come out.
//! It adds no right: a paired device already holds a full terminal as this
//! user, from which `leon king` says the same.
//!
//! Opening may start King's service and wait for it, and the socket is read
//! and written by blocking calls, so a connection lives on threads of its
//! own: the session hands it lines and never waits.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use leon_wire::{Message, MAX_KING_LINE};
use tokio::sync::mpsc;

/// What opens a connection to King's service on this computer.
pub trait KingDoor: Send + Sync {
    /// A new connection, or why there is none.
    fn open(&self) -> Result<KingPipe, String>;
}

/// One connection to King's service.
pub struct KingPipe {
    /// Where lines to King are written.
    pub write: Box<dyn Write + Send>,
    /// Where King's lines are read.
    pub read: Box<dyn BufRead + Send>,
    /// Ends the connection, so that a read waiting on it returns.
    pub close: Box<dyn Fn() + Send + Sync>,
}

/// The door a host is given, kept out of the configuration's `Debug`.
#[derive(Clone)]
pub struct King(pub Arc<dyn KingDoor>);

impl std::fmt::Debug for King {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KingDoor")
    }
}

/// One session's line to King: started by the first line it is given.
pub(crate) struct KingLine {
    door: Option<King>,
    out: mpsc::Sender<Message>,
    lines: Option<std::sync::mpsc::Sender<String>>,
}

impl KingLine {
    pub(crate) fn new(door: Option<King>, out: mpsc::Sender<Message>) -> Self {
        Self {
            door,
            out,
            lines: None,
        }
    }

    /// Passes a line on to King. Never waits.
    pub(crate) fn send(&mut self, line: String) {
        let Some(door) = self.door.clone() else {
            let _ = self.out.try_send(Message::KingClosed {
                reason: "this computer's Leon has no Orchestrator to speak to".to_owned(),
            });
            return;
        };
        let lines = self.lines.get_or_insert_with(|| {
            let (lines, waiting) = std::sync::mpsc::channel();
            let out = self.out.clone();
            std::thread::spawn(move || carry(&door, &waiting, &out));
            lines
        });
        let _ = lines.send(line);
    }
}

/// An open connection: where to write, how to end it, and whether its reader
/// still reads.
struct Open {
    write: Box<dyn Write + Send>,
    close: Box<dyn Fn() + Send + Sync>,
    reading: Arc<AtomicBool>,
}

impl Drop for Open {
    fn drop(&mut self) {
        (self.close)();
    }
}

/// Writes the session's lines to King, opening the connection when there is
/// none, until the session is gone.
fn carry(door: &King, waiting: &std::sync::mpsc::Receiver<String>, out: &mpsc::Sender<Message>) {
    let closed = |reason: String| {
        let _ = out.blocking_send(Message::KingClosed { reason });
    };
    let mut open: Option<Open> = None;
    while let Ok(line) = waiting.recv() {
        if open
            .as_ref()
            .is_some_and(|open| !open.reading.load(Ordering::Acquire))
        {
            open = None;
        }
        if open.is_none() {
            match door.0.open() {
                Ok(pipe) => {
                    let reading = Arc::new(AtomicBool::new(true));
                    let (read, out, flag) = (pipe.read, out.clone(), reading.clone());
                    std::thread::spawn(move || listen(read, &out, &flag));
                    open = Some(Open {
                        write: pipe.write,
                        close: pipe.close,
                        reading,
                    });
                }
                Err(reason) => {
                    closed(reason);
                    continue;
                }
            }
        }
        let Some(connection) = open.as_mut() else {
            continue;
        };
        let written = connection
            .write
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|()| connection.write.flush());
        if let Err(error) = written {
            open = None;
            closed(format!(
                "Orchestrator's service could not be written to: {error}"
            ));
        }
    }
}

/// Reads King's lines and passes them to the session, until the connection
/// ends.
fn listen(mut read: Box<dyn BufRead + Send>, out: &mpsc::Sender<Message>, reading: &AtomicBool) {
    let mut line = String::new();
    let reason = loop {
        line.clear();
        match read.read_line(&mut line) {
            Ok(0) => break "Orchestrator's service closed the line".to_owned(),
            Ok(_) => {}
            Err(error) => break format!("Orchestrator's service could not be read: {error}"),
        }
        let line = line.trim_end_matches(['\n', '\r']).to_owned();
        if line.len() > MAX_KING_LINE {
            break "Orchestrator sent more than one line may hold".to_owned();
        }
        if out.blocking_send(Message::King { line }).is_err() {
            // The session is gone.
            reading.store(false, Ordering::Release);
            return;
        }
    };
    reading.store(false, Ordering::Release);
    let _ = out.blocking_send(Message::KingClosed { reason });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;

    /// A door to a King that answers each line with `"echo: <line>"` and,
    /// once, a notice of its own.
    struct Echo {
        opened: Mutex<usize>,
    }

    impl KingDoor for Echo {
        fn open(&self) -> Result<KingPipe, String> {
            *self.opened.lock().unwrap() += 1;
            let (ours, theirs) = UnixStream::pair().map_err(|error| error.to_string())?;
            std::thread::spawn(move || {
                let mut answer = theirs.try_clone().unwrap();
                writeln!(answer, "notice").unwrap();
                for line in BufReader::new(theirs).lines() {
                    let Ok(line) = line else { break };
                    if line == "hang up" {
                        break;
                    }
                    writeln!(answer, "echo: {line}").unwrap();
                }
            });
            let closer = ours.try_clone().unwrap();
            Ok(KingPipe {
                write: Box::new(ours.try_clone().unwrap()),
                read: Box::new(BufReader::new(ours)),
                close: Box::new(move || {
                    let _ = closer.shutdown(std::net::Shutdown::Both);
                }),
            })
        }
    }

    async fn next(rx: &mut mpsc::Receiver<Message>) -> Message {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a message in time")
            .expect("the line is open")
    }

    fn king(line: &str) -> Message {
        Message::King { line: line.into() }
    }

    #[tokio::test]
    async fn lines_go_to_king_and_back_and_a_closed_line_opens_again() {
        let door = Arc::new(Echo {
            opened: Mutex::new(0),
        });
        let (out, mut rx) = mpsc::channel(16);
        let mut line = KingLine::new(Some(King(door.clone())), out);
        line.send("status".into());
        assert_eq!(next(&mut rx).await, king("notice"));
        assert_eq!(next(&mut rx).await, king("echo: status"));
        line.send("hang up".into());
        assert!(matches!(next(&mut rx).await, Message::KingClosed { .. }));
        line.send("again".into());
        assert_eq!(next(&mut rx).await, king("notice"));
        assert_eq!(next(&mut rx).await, king("echo: again"));
        assert_eq!(*door.opened.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn a_host_without_king_says_so() {
        let (out, mut rx) = mpsc::channel(4);
        KingLine::new(None, out).send("status".into());
        assert!(matches!(
            next(&mut rx).await,
            Message::KingClosed { reason } if reason.contains("no Orchestrator")
        ));
    }
}
