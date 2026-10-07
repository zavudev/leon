//! Keeping the tree's history current while agents run in Leon.
//!
//! A session started in Leon should be a row of the tree within seconds, and
//! restoring needs its id, so:
//!
//! * **Importing promptly** ([`Shell::request_import`]): an incremental import
//!   (`Op::SyncHistory`, off the UI thread, by the importer's own cursors and
//!   each source's change stamp) is asked for, after a short pause that merges
//!   bursts, when an agent starts, goes quiet after output, or ends, when the
//!   window regains the focus, and on a coarse timer.
//! * **Learning the id** ([`Shell::learn_session_ids`]): a terminal started
//!   fresh is matched to the session that appears for it (see `crate::learn`)
//!   and the link is kept, so the tree shows one row (live now, history
//!   later) and a restore can resume it. A terminal whose program moves to
//!   another session inside it is relinked by the title the program sets.

use super::live::LiveId;
use super::shell::Shell;
use crate::engine::Op;
use crate::learn::{self, Fresh, Taken};
use chrono::{DateTime, Utc};
use gpui_kit::{Context, Task, Window};
use leon_core::{MachineId, SessionFilter};

/// What the window keeps about importing.
#[derive(Default)]
pub struct SyncUi {
    waiting: Option<Task<()>>,
    ticker: Option<Task<()>>,
}

impl Shell {
    /// Asks for an incremental import after the pause of
    /// `Options::import_debounce`; asking again meanwhile changes nothing.
    pub(super) fn request_import(&mut self, cx: &mut Context<Self>) {
        if self.sync.waiting.is_some() {
            return;
        }
        let pause = self.options.import_debounce;
        self.sync.waiting = Some(cx.spawn(async move |this, cx| {
            if !pause.is_zero() {
                cx.background_executor().timer(pause).await;
            }
            this.update(cx, |this, cx| {
                this.sync.waiting = None;
                this.engine.submit(Op::SyncHistory);
                // What the process scan says helps to learn the ids.
                this.scan_elsewhere_now(false, cx);
            })
            .ok();
        }));
    }

    /// The coarse timer of imports, while the window lives.
    pub(super) fn watch_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let every = self.options.import_interval;
        if every.is_zero() {
            return;
        }
        let _ = window;
        self.sync.ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(every).await;
            if this.update(cx, |this, cx| this.request_import(cx)).is_err() {
                return;
            }
        }));
    }

    /// Links the terminals started fresh to the sessions that appeared for
    /// them, and a terminal whose program moved to another session to that
    /// one. Cheap when there is nothing to learn.
    pub(super) fn learn_session_ids(&mut self, cx: &mut Context<Self>) {
        let local = MachineId::local();
        let mut taken: Taken = self
            .live
            .all()
            .iter()
            .filter_map(|s| Some((s.agent?, s.learned.as_ref()?.0.clone())))
            .collect();
        let fresh: Vec<Fresh> = self
            .live
            .all()
            .iter()
            .filter(|s| {
                s.machine == local
                    && s.shown_agent().is_some()
                    && s.learned.is_none()
                    && !s.is_paused()
            })
            .filter_map(|s| {
                Some(Fresh {
                    id: s.id.0,
                    agent: s.shown_agent()?,
                    cwd: s.cwd.clone(),
                    started: DateTime::<Utc>::from_timestamp_millis(s.started_ms)?,
                    shell_pid: s.view.read(cx).terminal().process_id(),
                })
            })
            .collect();
        let mut learned = Vec::new();
        if !fresh.is_empty() {
            let found = self
                .engine
                .elsewhere(&local)
                .map(|report| report.found.clone())
                .unwrap_or_default();
            learned = learn::by_process(&fresh, &found, &taken);
            taken.extend(learned.iter().filter_map(|l| {
                let agent = fresh.iter().find(|f| f.id == l.id)?.agent;
                Some((agent, l.external.clone()))
            }));
            let rest: Vec<Fresh> = fresh
                .into_iter()
                .filter(|f| !learned.iter().any(|l| l.id == f.id))
                .collect();
            if !rest.is_empty() {
                let sessions = self
                    .engine
                    .store()
                    .recent_sessions(
                        &SessionFilter {
                            machine_id: Some(local),
                            ..Default::default()
                        },
                        200,
                    )
                    .unwrap_or_default();
                learned.extend(learn::by_folder(&rest, &sessions, &taken));
            }
        }
        // A terminal an agent moved to another session in: the title the
        // program set names the one on screen, so the link follows it. The
        // link is what makes the tree show one row and what makes opening
        // the session land on the terminal instead of starting a second
        // agent on it; without this it lags behind for good.
        let titled: Vec<learn::Titled> = self
            .live
            .all()
            .iter()
            .filter(|s| s.shown_agent().is_some() && s.learned.is_some() && !s.is_paused())
            .filter_map(|s| {
                Some(learn::Titled {
                    id: s.id.0,
                    machine: s.machine.clone(),
                    agent: s.shown_agent()?,
                    cwd: s.cwd.clone(),
                    title: s.title.clone()?,
                })
            })
            .collect();
        if !titled.is_empty() {
            learned.extend(learn::by_title(&titled, &self.snapshot.sessions, &taken));
        }
        if learned.is_empty() {
            return;
        }
        for item in learned {
            // A history row is one terminal's: never two.
            let history = item
                .history
                .filter(|row| self.live.of_history(row).is_none());
            if let Some(session) = self.live.get_mut(LiveId(item.id)) {
                session.learned = Some((item.external, item.how.to_owned()));
                if let Some(row) = history {
                    // A fresh link fills what was empty; a title follows the
                    // program, which may have moved on from what was linked.
                    if session.history.is_none() || item.how == "title" {
                        session.history = Some(row);
                    }
                }
            }
        }
        self.refresh_live();
        cx.notify();
    }
}
