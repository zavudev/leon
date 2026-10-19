//! The window's line to King's service.
//!
//! The window is one more client of the control socket, as `leon king …` is:
//! it asks what it shows and is told when anything changes. A [`Link`] opens
//! a [`Connection`]: requests go in on one channel, tagged by the window, and
//! the replies (with the same tag), the notices and the end of the line come
//! out on the other. The real link ([`Socket`]) starts the service when
//! nothing answers, as every client does, and keeps one thread that writes
//! and one that reads; tests give the window a link whose other end they
//! play themselves ([`testing::Fake`]).

use leon_king::protocol::{Notice, Request};
use serde_json::Value;

/// Opens connections to King's service.
pub trait Link: Send + Sync {
    /// A new connection, subscribed to the notices. It may wait: the window
    /// calls it off its own thread.
    fn open(&self) -> Result<Connection, String>;
}

/// One connection, as two channels.
pub struct Connection {
    /// What the window asks, with the tag its reply will carry.
    pub ask: flume::Sender<(u64, Request)>,
    /// What the service says.
    pub heard: flume::Receiver<Heard>,
}

/// What the service said.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// The answer to the request with this tag.
    Reply {
        /// The request's tag.
        tag: u64,
        /// What it gave, or why it was refused.
        result: Result<Value, String>,
    },
    /// Something changed, or something is for the person.
    Notice(Notice),
    /// The line is gone; nothing more comes on it.
    Lost(String),
}

/// The control socket of this computer's service.
#[cfg(unix)]
pub struct Socket {
    /// Where the service is.
    pub paths: super::Paths,
}

#[cfg(unix)]
impl Link for Socket {
    fn open(&self) -> Result<Connection, String> {
        use super::client::{Client, Incoming};

        let client = Client::dial_or_start(&self.paths)?;
        let (mut writer, mut reader) = client.split();
        // Tag 0 is never the window's: the subscription's reply is dropped.
        writer.send_as(0, Request::Subscribe)?;
        let (ask, asked) = flume::unbounded::<(u64, Request)>();
        let (tell, heard) = flume::unbounded();
        std::thread::Builder::new()
            .name("king-link-write".into())
            .spawn(move || {
                while let Ok((tag, request)) = asked.recv() {
                    if writer.send_as(tag, request).is_err() {
                        return;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        std::thread::Builder::new()
            .name("king-link-read".into())
            .spawn(move || loop {
                let said = match reader.read() {
                    Ok(Incoming::Reply(reply)) if reply.id == 0 => continue,
                    Ok(Incoming::Reply(reply)) => Heard::Reply {
                        tag: reply.id,
                        result: match reply.error {
                            Some(error) if !reply.ok => Err(error),
                            _ => Ok(reply.result),
                        },
                    },
                    Ok(Incoming::Notice(notice)) => Heard::Notice(notice),
                    Err(error) => {
                        let _ = tell.send(Heard::Lost(error));
                        return;
                    }
                };
                if tell.send(said).is_err() {
                    return;
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Connection { ask, heard })
    }
}

/// A link whose other end a test plays. The tests that play it are POSIX
/// ones (`ui/tests_king.rs`).
#[cfg(test)]
#[cfg_attr(not(leon_posix_tests), allow(dead_code))]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Each connection opened hands its other end to the test.
    #[derive(Clone, Default)]
    pub struct Fake {
        ends: Arc<Mutex<Vec<End>>>,
        refuse: Arc<Mutex<Option<String>>>,
    }

    /// The service's end of one connection.
    pub struct End {
        /// What the window asked.
        pub asked: flume::Receiver<(u64, Request)>,
        /// What the test says to the window.
        pub tell: flume::Sender<Heard>,
    }

    impl Fake {
        /// The ends of the connections opened so far, taken.
        pub fn take(&self) -> Vec<End> {
            std::mem::take(&mut self.ends.lock().unwrap())
        }

        /// The next connection is refused with `why`.
        pub fn refuse(&self, why: &str) {
            *self.refuse.lock().unwrap() = Some(why.to_owned());
        }
    }

    impl Link for Fake {
        fn open(&self) -> Result<Connection, String> {
            if let Some(why) = self.refuse.lock().unwrap().take() {
                return Err(why);
            }
            let (ask, asked) = flume::unbounded();
            let (tell, heard) = flume::unbounded();
            self.ends.lock().unwrap().push(End { asked, tell });
            Ok(Connection { ask, heard })
        }
    }

    impl End {
        /// Everything the window asked so far.
        pub fn drain(&self) -> Vec<(u64, Request)> {
            self.asked.try_iter().collect()
        }
    }
}
