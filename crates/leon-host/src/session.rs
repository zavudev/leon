//! One client's session on the host.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use leon_link::SecureChannel;
use leon_wire::{
    ErrorCode, Message, SharedProject, SharedSession, WireError, PROTOCOL_VERSION,
    SHARE_SESSIONS_LIMIT,
};
use tokio::sync::{mpsc, Notify};

use crate::exec;
use crate::host::{Inner, Shared};
use crate::ptys::Outbox;
use crate::share::{ShareSource, TRANSCRIPT_BUDGET_BYTES};

/// Commands one session may run at the same time.
const MAX_EXECS: usize = 8;
/// Messages queued for one client.
const OUTBOX: usize = 1024;

fn error(id: Option<u64>, code: ErrorCode, message: &str) -> Message {
    Message::Error {
        id,
        error: WireError {
            code,
            message: message.into(),
        },
    }
}

struct Counter(Arc<AtomicUsize>);

impl Counter {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter.clone())
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) async fn serve(inner: Arc<Inner>, mut channel: SecureChannel) {
    let key = channel.remote_static();
    let _open = Counter::new(&inner.sessions);
    let hello = tokio::time::timeout(Duration::from_secs(10), channel.recv()).await;
    let Ok(Ok(Some(Message::Hello {
        protocol,
        device_name,
        ..
    }))) = hello
    else {
        return;
    };
    if protocol != PROTOCOL_VERSION {
        let _ = channel
            .send(&error(
                None,
                ErrorCode::Unsupported,
                "unsupported protocol version",
            ))
            .await;
        return;
    }
    let ours = Message::Hello {
        protocol: PROTOCOL_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        device_name: inner.cfg.device_name.clone(),
        token: None,
    };
    if channel.send(&ours).await.is_err() {
        return;
    }
    let _ = inner.registry.lock().touch(&key, Inner::now_unix());
    tracing::info!(device = %leon_link::DeviceId::of(&key), name = %device_name, "session opened");

    let session = inner.next_session.fetch_add(1, Ordering::Relaxed);
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(OUTBOX);
    let lagged = Arc::new(Notify::new());
    let outbox = Outbox {
        session,
        tx: out_tx.clone(),
        lagged: lagged.clone(),
    };
    let execs = Arc::new(AtomicUsize::new(0));
    let mut recheck = inner.recheck.subscribe();

    'session: loop {
        tokio::select! {
            incoming = channel.recv() => match incoming {
                Ok(Some(message)) => {
                    if !handle(&inner, message, &outbox, &out_tx, &execs) {
                        break 'session;
                    }
                }
                Ok(None) | Err(_) => break 'session,
            },
            outgoing = out_rx.recv() => match outgoing {
                Some(message) => {
                    if channel.send(&message).await.is_err() {
                        break 'session;
                    }
                }
                None => break 'session,
            },
            _ = lagged.notified() => {
                let _ = channel.send(&error(None, ErrorCode::Busy, "the connection could not keep up; reconnect")).await;
                break 'session;
            }
            changed = recheck.recv() => {
                if !matches!(changed, Err(tokio::sync::broadcast::error::RecvError::Closed))
                    && !inner.registry.lock().is_authorised(&key)
                {
                    let _ = channel.send(&error(None, ErrorCode::Forbidden, "this device was revoked")).await;
                    break 'session;
                }
            }
        }
    }
    inner.table.detach_session(session);
    tracing::info!(name = %device_name, "session closed");
}

/// Handles one request; `false` ends the session.
fn handle(
    inner: &Arc<Inner>,
    message: Message,
    outbox: &Outbox,
    out_tx: &mpsc::Sender<Message>,
    execs: &Arc<AtomicUsize>,
) -> bool {
    let reply = |m: Message| out_tx.try_send(m).is_ok();
    match message {
        Message::Ping { nonce } => reply(Message::Pong { nonce }),
        Message::Exec {
            id,
            spec,
            timeout_ms,
        } => {
            if execs.load(Ordering::Relaxed) >= MAX_EXECS {
                return reply(error(
                    Some(id),
                    ErrorCode::Busy,
                    "too many commands at once",
                ));
            }
            let (guard, out) = (Counter::new(execs), out_tx.clone());
            tokio::spawn(async move {
                let message = match exec::run(&spec, timeout_ms).await {
                    Ok(output) => Message::ExecOutput { id, output },
                    Err(error) => Message::Error {
                        id: Some(id),
                        error,
                    },
                };
                drop(guard);
                let _ = out.send(message).await;
            });
            true
        }
        Message::PtyOpen {
            id,
            spec,
            size,
            label,
        } => match inner.table.open(&spec, size, &label) {
            Ok(pty) => {
                if !reply(Message::PtyOpened { id, pty }) {
                    return false;
                }
                match inner.table.attach(pty, 0, outbox, None) {
                    Ok(_) => true,
                    Err(e) => reply(Message::Error {
                        id: Some(id),
                        error: e,
                    }),
                }
            }
            Err(e) => reply(Message::Error {
                id: Some(id),
                error: e,
            }),
        },
        Message::PtyData { pty, bytes, .. } => match inner.table.write(pty, bytes) {
            Ok(()) => true,
            Err(e) => reply(Message::Error { id: None, error: e }),
        },
        Message::PtyResize { pty, size } => match inner.table.resize(pty, size) {
            Ok(()) => true,
            Err(e) => reply(Message::Error { id: None, error: e }),
        },
        Message::PtyClose { pty } => match inner.table.close(pty) {
            Ok(()) => true,
            Err(e) => reply(Message::Error { id: None, error: e }),
        },
        Message::PtyList { id } => reply(Message::PtyListing {
            id,
            ptys: inner.table.list(),
        }),
        Message::PtyAttach {
            id,
            pty,
            from_offset,
        } => match inner.table.attach(pty, from_offset, outbox, Some(id)) {
            Ok(_) => true,
            Err(e) => reply(Message::Error {
                id: Some(id),
                error: e,
            }),
        },
        Message::ShareState { id } => share_request(
            inner.cfg.share.clone(),
            id,
            out_tx.clone(),
            move |share| {
                let projects: Vec<SharedProject> = share.projects();
                let mut sessions: Vec<SharedSession> = share.sessions();
                let truncated = sessions.len() > SHARE_SESSIONS_LIMIT;
                sessions.truncate(SHARE_SESSIONS_LIMIT);
                Message::ShareStateData {
                    id,
                    projects,
                    sessions,
                    truncated,
                }
            },
            move || Message::ShareStateData {
                id,
                projects: Vec::new(),
                sessions: Vec::new(),
                truncated: false,
            },
        ),
        Message::ShareTranscript {
            id,
            agent,
            external_id,
        } => share_request(
            inner.cfg.share.clone(),
            id,
            out_tx.clone(),
            move |share| {
                let transcript = share
                    .transcript(&agent, &external_id)
                    .map(|mut transcript| {
                        transcript.messages = crate::share::bound_transcript(
                            &transcript.messages,
                            TRANSCRIPT_BUDGET_BYTES,
                        );
                        transcript
                    });
                Message::ShareTranscriptData { id, transcript }
            },
            move || Message::ShareTranscriptData {
                id,
                transcript: None,
            },
        ),
        // Responses and repeated greetings are not requests.
        _ => reply(error(None, ErrorCode::BadRequest, "unexpected message")),
    }
}

/// Answers a share request on the blocking pool: what the share source reads
/// (this Leon's own store and the agents' files) can take longer than an
/// async turn should. `share` is `None` when nothing is shared: `idle` then
/// answers with the request's empty result, so a client asking an old or a
/// bare `leon host` is told "nothing to see" and never an error.
fn share_request(
    share: Option<Shared>,
    id: u64,
    out: mpsc::Sender<Message>,
    make: impl FnOnce(&dyn ShareSource) -> Message + Send + 'static,
    idle: impl FnOnce() -> Message + Send + 'static,
) -> bool {
    match share {
        Some(Shared(share)) => {
            tokio::spawn(async move {
                let message = tokio::task::spawn_blocking(move || make(share.as_ref()))
                    .await
                    .unwrap_or_else(|_| error(Some(id), ErrorCode::Internal, "the share failed"));
                let _ = out.send(message).await;
            });
            true
        }
        None => {
            let _ = out.try_send(idle());
            true
        }
    }
}
