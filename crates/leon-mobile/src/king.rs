//! King Leon from the phone: the coordinator of the paired computer, spoken
//! to through its host.
//!
//! King's service there speaks one JSON request per line and answers with
//! one reply per line (`leon_king::protocol`); a client that subscribed also
//! reads its notices. The host passes the lines through, one connection to
//! King per session (`leon_host::king`). This module is the phone's end:
//!
//! * [`HostSession::king_ask`] sends one request and waits for its reply;
//! * [`HostSession::king_watch`] subscribes and tells a [`KingListener`]
//!   every notice, subscribing again after the line closed (King's service
//!   restarted, or the connection to the computer dropped).
//!
//! What crosses to the application is the protocol's own JSON: the requests
//! and the notices are King's, and the application reads them as they are.
//! Only a computer whose [`crate::session::SharedState::king`] is set may be
//! asked; one that does not know the message drops the connection.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use leon_link::client::{Client, KingEvent};
use serde_json::{Map, Value};
use tokio::sync::broadcast::error::RecvError;

use crate::error::MobileError;
use crate::runtime::{on_runtime, runtime};
use crate::session::HostSession;

/// How long a request may wait for King's reply.
const ASK_WITHIN: Duration = Duration::from_secs(30);
/// How long a watch waits before it subscribes again after the line closed.
const SUBSCRIBE_AGAIN_AFTER: Duration = Duration::from_secs(2);
/// How long a watch waits for the connection before it looks again.
const ONLINE_WITHIN: Duration = Duration::from_secs(60);

/// The ids of the requests sent to King: one count for every computer,
/// which keeps them apart on each connection too.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Told what King's service says of itself while a watch lasts.
#[uniffi::export(callback_interface)]
pub trait KingListener: Send + Sync {
    /// A notice of King's service, as its JSON: `{"notice":"changed",…}`
    /// when King's state changed (its status rides along), or
    /// `{"notice":"notify",…}` when King has something to tell the person.
    fn on_notice(&self, notice: String);
    /// The line to King closed: what it said since is not known. The watch
    /// subscribes again by itself; ask for the status to catch up.
    fn on_closed(&self, reason: String);
}

/// A watch on King's notices ([`HostSession::king_watch`]).
#[derive(uniffi::Object)]
pub struct KingWatch {
    stopped: Arc<AtomicBool>,
}

#[uniffi::export]
impl KingWatch {
    /// Ends the watch: no notice is told after this returns.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}

/// A request as one line, with its id: `request` is a JSON object with its
/// `op` (`{"op":"say","text":"…"}`).
fn line_of(request: &str, id: u64) -> Result<String, MobileError> {
    let mut object: Map<String, Value> =
        serde_json::from_str(request).map_err(|_| MobileError::Protocol)?;
    if !object.get("op").is_some_and(Value::is_string) {
        return Err(MobileError::Protocol);
    }
    object.insert("id".to_owned(), Value::from(id));
    serde_json::to_string(&object).map_err(|_| MobileError::Protocol)
}

/// What a line of King is: the reply to `id` (its result, or the error it
/// gives), a notice, or something else.
enum Heard {
    Reply(Result<String, MobileError>),
    Notice,
    Other,
}

fn heard(line: &str, id: u64) -> Heard {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Heard::Other;
    };
    if value.get("notice").is_some() {
        return Heard::Notice;
    }
    if value.get("id").and_then(Value::as_u64) != Some(id) {
        return Heard::Other;
    }
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        let result = value.get("result").cloned().unwrap_or(Value::Null);
        return Heard::Reply(Ok(result.to_string()));
    }
    let message = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("Orchestrator could not do it")
        .to_owned();
    Heard::Reply(Err(MobileError::Host { message }))
}

/// Sends `request` and waits for its reply.
async fn ask(client: &Client, request: &str) -> Result<String, MobileError> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let line = line_of(request, id)?;
    // Listening before sending, so the reply cannot come first.
    let mut events = client.king_events();
    client.king_send(line);
    let wait = async {
        loop {
            match events.recv().await {
                Ok(KingEvent::Line(line)) => {
                    if let Heard::Reply(result) = heard(&line, id) {
                        return result;
                    }
                }
                Ok(KingEvent::Closed(message)) => return Err(MobileError::Host { message }),
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Err(MobileError::Closed),
            }
        }
    };
    tokio::time::timeout(ASK_WITHIN, wait)
        .await
        .unwrap_or(Err(MobileError::Timeout))
}

/// Subscribes, tells the notices, and subscribes again after the line
/// closed, until the watch is stopped or the session closed.
async fn watch(client: Client, listener: Box<dyn KingListener>, stopped: Arc<AtomicBool>) {
    let stopped_now = || stopped.load(Ordering::Acquire);
    while !stopped_now() {
        if client.wait_online(ONLINE_WITHIN).await.is_err() {
            if client.state() == leon_link::client::ConnState::Closed {
                return;
            }
            continue;
        }
        let mut events = client.king_events();
        let reason = match ask(&client, r#"{"op":"subscribe"}"#).await {
            Err(error) => error.to_string(),
            Ok(_) => loop {
                let event = tokio::select! {
                    event = events.recv() => event,
                    () = tokio::time::sleep(Duration::from_millis(500)) => {
                        if stopped_now() {
                            return;
                        }
                        continue;
                    }
                };
                match event {
                    Ok(KingEvent::Line(line)) => {
                        if !stopped_now() && matches!(heard(&line, 0), Heard::Notice) {
                            listener.on_notice(line);
                        }
                    }
                    Ok(KingEvent::Closed(reason)) => break reason,
                    Err(RecvError::Lagged(_)) => {
                        break "some of Orchestrator's notices were missed".to_owned();
                    }
                    Err(RecvError::Closed) => return,
                }
            },
        };
        if stopped_now() {
            return;
        }
        listener.on_closed(reason);
        tokio::time::sleep(SUBSCRIBE_AGAIN_AFTER).await;
    }
}

#[uniffi::export]
impl HostSession {
    /// Asks King's service on the other computer and returns its answer.
    /// `request` is one request of King's protocol as JSON, with its `op`:
    /// `{"op":"status"}`, `{"op":"say","text":"…"}`,
    /// `{"op":"transcript","target":"brain","since":12}`. The answer is the
    /// reply's `result`, as JSON; a refusal is [`MobileError::Host`] with
    /// King's sentence.
    pub async fn king_ask(&self, request: String) -> Result<String, MobileError> {
        let client = self.client().clone();
        on_runtime(async move { ask(&client, &request).await }).await
    }

    /// Tells `listener` every notice of King's service from now on, until
    /// [`KingWatch::stop`].
    pub fn king_watch(&self, listener: Box<dyn KingListener>) -> Arc<KingWatch> {
        let stopped = Arc::new(AtomicBool::new(false));
        let client = self.client().clone();
        runtime().spawn(watch(client, listener, stopped.clone()));
        Arc::new(KingWatch { stopped })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_gets_its_id_and_must_name_what_it_asks() {
        let line = line_of(r#"{"op":"say","text":"hi"}"#, 7).unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 7);
        assert_eq!(value["op"], "say");
        assert_eq!(line_of(r#"{"text":"hi"}"#, 7), Err(MobileError::Protocol));
        assert_eq!(line_of("say hi", 7), Err(MobileError::Protocol));
    }

    #[test]
    fn a_reply_is_told_from_a_notice_and_from_another_reply() {
        let ok = heard(r#"{"id":3,"ok":true,"result":{"busy":false}}"#, 3);
        assert!(matches!(ok, Heard::Reply(Ok(result)) if result == r#"{"busy":false}"#));
        let refused = heard(r#"{"id":3,"ok":false,"error":"no such worker"}"#, 3);
        assert!(matches!(
            refused,
            Heard::Reply(Err(MobileError::Host { message })) if message == "no such worker"
        ));
        assert!(matches!(
            heard(r#"{"id":4,"ok":true,"result":null}"#, 3),
            Heard::Other
        ));
        assert!(matches!(heard(r#"{"notice":"changed"}"#, 3), Heard::Notice));
    }
}
