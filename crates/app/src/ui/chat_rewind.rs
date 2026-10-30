//! "Edit from here": taking a conversation back to before one of the
//! person's messages, and putting the message back in the field.
//!
//! Hovering a message the person sent shows two actions: *Edit* puts its
//! words (and files) back in the field, *Retry* sends them again. Both first
//! go back to before that message, which is asked in a small cell above the
//! field: with the files of the worktree put back as they were when the turn
//! began (`crate::chat::checkpoint`), or kept as they are. Neither is offered
//! while a turn runs. What the agent itself forgets is each agent's own way
//! (`crate::chat::rewind`).
//!
//! Files are put back only where nothing else works: another conversation or
//! agent in the same folder would lose what it did, so there the cell offers
//! only to keep them.

use gpui_kit::prelude::*;
use gpui_kit::{Context, Window};
use leon_core::MachineKind;
use leon_turn::TurnSpec;

use super::chat::Chat;
use super::live::LiveId;
use super::shell::Shell;
use crate::chat::model::{Item, Tone};
use crate::chat::rewind::{self, Job, Mark};
use crate::chat::turn::Start;
use crate::engine::StatusKind;

/// A rewind that was asked for and waits for the person to choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rewinding {
    /// Where the message is among the conversation's items.
    pub at: usize,
    /// Whether the message is sent again afterwards (retry), instead of put
    /// back in the field (edit).
    pub resend: bool,
    /// The going back is under way.
    pub busy: bool,
}

/// What can be done about the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Files {
    /// They can be put back as they were.
    Offered,
    /// Nothing was kept of them: the folder is not a git worktree, or it is
    /// on another machine, or the snapshot failed.
    NoCheckpoint,
    /// Something else works in the folder: they stay as they are.
    Shared,
}

impl Chat {
    /// The turn the person's words at `at` began, if that is what they are.
    pub fn mark_at(&self, at: usize) -> Option<&Mark> {
        self.marks.iter().find(|mark| mark.item == at)
    }

    /// Whether the message at `at` can be gone back to: it began a turn,
    /// nothing is going on, and the agent that has the conversation now is
    /// the one it was said to (a message before a switch of agents was said
    /// to a session that is let go).
    pub fn can_rewind(&self, at: usize) -> bool {
        !self.conversation.running()
            && !self.launching
            && self.rewinding.is_none_or(|asked| !asked.busy)
            && self.mark_at(at).is_some()
            && !self.rewind_blocked_by_switch(at)
    }
}

impl Shell {
    /// What can be done about the files if the conversation goes back to
    /// before the message at `at`.
    pub(super) fn chat_rewind_files(&self, id: LiveId, at: usize) -> Files {
        let Some(chat) = self.chats.get(&id) else {
            return Files::NoCheckpoint;
        };
        if chat
            .mark_at(at)
            .is_none_or(|mark| mark.checkpoint.is_none())
        {
            return Files::NoCheckpoint;
        }
        let near = |machine: &leon_core::MachineId, cwd: &str| {
            machine == &chat.machine
                && (leon_core::path::is_within(cwd, &chat.cwd)
                    || leon_core::path::is_within(&chat.cwd, cwd))
        };
        let other_chat = self
            .chats
            .iter()
            .any(|(other, chat)| *other != id && near(&chat.machine, &chat.cwd));
        let other_agent = self
            .live
            .all()
            .iter()
            .any(|live| live.agent.is_some() && near(&live.machine, &live.cwd));
        if other_chat || other_agent {
            Files::Shared
        } else {
            Files::Offered
        }
    }

    /// The person asks to go back to before the message at `at`: the cell
    /// that asks what to do about the files opens.
    pub(super) fn chat_rewind_ask(
        &mut self,
        id: LiveId,
        at: usize,
        resend: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if !chat.can_rewind(at) {
            return;
        }
        chat.rewinding = Some(Rewinding {
            at,
            resend,
            busy: false,
        });
        cx.notify();
    }

    /// The person changed their mind.
    pub(super) fn chat_rewind_cancel(&mut self, id: LiveId, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        if chat.rewinding.is_some_and(|asked| !asked.busy) {
            chat.rewinding = None;
            cx.notify();
        }
    }

    /// Goes back to before the message that was asked about, with the files
    /// put back as they were or as they are.
    pub(super) fn chat_rewind(
        &mut self,
        id: LiveId,
        files: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(asked) = self
            .chats
            .get(&id)
            .and_then(|chat| chat.rewinding)
            .filter(|asked| !asked.busy)
        else {
            return;
        };
        let restore = files && self.chat_rewind_files(id, asked.at) == Files::Offered;
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let (Some(adapter), Some(mark)) = (
            crate::chat::adapter_of(chat.agent),
            chat.mark_at(asked.at).cloned(),
        ) else {
            return;
        };
        let Some(machine) = self.snapshot.machine(&chat.machine).cloned() else {
            return;
        };
        let program = self.chat_program(chat.agent, cx);
        let spec = TurnSpec {
            program: &program,
            cwd: Some(&chat.cwd),
            ..TurnSpec::default()
        };
        let job = Job {
            how: adapter.rewind,
            agent: crate::format::agent_name(chat.agent).to_owned(),
            memory: mark.memory.clone(),
            session: chat.conversation.session_id.clone(),
            turns: rewind::turns_reached(&chat.conversation.items, &chat.marks, asked.at),
            projects: self.engine.prefs().roots.claude_projects,
            cwd: chat.cwd.clone(),
            start: Start {
                argv: (adapter.argv)(&spec),
                env: self.chat_env(&machine, chat.agent, chat.account.as_deref(), &chat.cwd, cx),
                cwd: chat.cwd.clone(),
                machine: machine.clone(),
            },
            restore: restore.then(|| mark.checkpoint.clone()).flatten(),
            chat: chat.key.clone(),
            turn: mark.turn,
        };
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.rewinding = Some(Rewinding {
                busy: true,
                ..asked
            });
        }
        let spawner = self.options.chat_spawner.clone();
        // Files of a repository on this computer are read and written here.
        let local = matches!(machine.kind, MachineKind::Local);
        cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_spawn(async move {
                    if !local && job.restore.is_some() {
                        return Err("Files can only be put back on this computer.".to_owned());
                    }
                    rewind::run(&job, &*spawner)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.chat_rewound(id, asked, done, window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The conversation went back (or could not): the transcript is cut, the
    /// message is where it was asked to be.
    fn chat_rewound(
        &mut self,
        id: LiveId,
        asked: Rewinding,
        done: Result<rewind::Done, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get_mut(&id) else {
            return;
        };
        chat.rewinding = None;
        let done = match done {
            Ok(done) => done,
            Err(why) => {
                self.engine.report(
                    StatusKind::Error,
                    format!("The conversation could not go back: {why}"),
                );
                cx.notify();
                return;
            }
        };
        let Some(Item::User { text, .. }) = chat.conversation.items.get(asked.at).cloned() else {
            cx.notify();
            return;
        };
        let before = (chat.state(), chat.activity(), chat.label());
        let new_session = done.session != chat.conversation.session_id;
        // The next message goes with what the new agent is told again when
        // the turn that took it is gone.
        let gone = chat.mark_at(asked.at).map(|mark| mark.turn);
        if let (Some(handoff), Some(gone)) = (chat.handoff.as_mut(), gone) {
            if handoff.sent_in.is_some_and(|sent| sent >= gone) {
                handoff.sent_in = None;
            }
        }
        chat.marks.retain(|mark| mark.item < asked.at);
        // The turns that are gone have nothing to count, and a panel that
        // showed one has nothing to show.
        let kept: Vec<u32> = chat.marks.iter().map(|mark| mark.turn).collect();
        chat.measured.retain(|turn, _| kept.contains(turn));
        if chat
            .diff
            .as_ref()
            .is_some_and(|panel| matches!(panel.source, super::chat_diff::Source::Turn(turn) if !kept.contains(&turn)))
        {
            chat.diff = None;
            chat.diff_reads += 1;
        }
        chat.open_work.retain(|from| *from < asked.at);
        chat.conversation.rewind(asked.at, done.session);
        if new_session {
            // The history has no row for a session that was just made.
            chat.history = None;
        }
        let notices = done
            .notice
            .map(|text| (Tone::Plain, text))
            .into_iter()
            .chain(done.files.map(|files| match files {
                Ok(restored) => (Tone::Plain, files_said(restored)),
                Err(why) => (Tone::Bad, format!("The files could not be put back: {why}")),
            }));
        for (tone, text) in notices {
            chat.conversation.items.push(Item::Notice { tone, text });
        }
        chat.seen = true;
        chat.scroll.scroll_to_bottom();
        if asked.resend {
            self.chat_say(id, text, cx);
        } else {
            let (words, files) = super::king::split_attached(&text);
            if let Some(chat) = self.chats.get_mut(&id) {
                chat.attached = files;
                chat.input.update(cx, |input, cx| {
                    input.set_value(&words, window, cx);
                    input.focus(window, cx);
                });
            }
        }
        if let Some(chat) = self.chats.get(&id) {
            if before != (chat.state(), chat.activity(), chat.label()) {
                self.rebuild_rows();
            }
        }
        cx.notify();
    }

    /// Forgets the checkpoints of a conversation that was closed, off the
    /// interface thread.
    pub(super) fn chat_forget_checkpoints(&self, chat: &Chat, cx: &mut Context<Self>) {
        if !chat.machine.is_local() || !crate::chat::checkpoint::in_worktree(chat_cwd(chat)) {
            return;
        }
        let (key, cwd) = (chat.key.clone(), chat.cwd.clone());
        cx.background_spawn(async move {
            if let Some(repo) = crate::chat::checkpoint::Repo::of(std::path::Path::new(&cwd)) {
                repo.forget(&key);
            }
        })
        .detach();
    }
}

fn chat_cwd(chat: &Chat) -> &std::path::Path {
    std::path::Path::new(&chat.cwd)
}

/// What is said of the files that were put back.
fn files_said(restored: crate::chat::checkpoint::Restored) -> String {
    let count = |n: usize| {
        if n == 1 {
            "1 file".to_owned()
        } else {
            format!("{n} files")
        }
    };
    match (restored.written, restored.removed) {
        (0, 0) => "The files were already as they were.".to_owned(),
        (written, 0) => format!("Files put back as they were: {} restored.", count(written)),
        (0, removed) => format!("Files put back as they were: {} removed.", count(removed)),
        (written, removed) => format!(
            "Files put back as they were: {} restored, {} removed.",
            count(written),
            count(removed)
        ),
    }
}
