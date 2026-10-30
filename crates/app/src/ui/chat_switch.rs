//! Going on with another agent in the same conversation.
//!
//! The chip of the agent (and `/agent`) lists the other agents installed on
//! the conversation's machine. Choosing one ends nothing that runs (a
//! conversation is a turn at a time, and the person cannot switch while one
//! is going): the session of the first agent is let go, a new one of the
//! other starts with the next message in the same folder, and that message
//! carries what was said before, written from Leon's own transcript
//! (`crate::chat::handoff`), before the person's words. The conversation
//! shows a rule where it happened, and the model, effort and mode are the
//! new agent's own again.
//!
//! What came before the rule is the first agent's: going back to before one
//! of its messages is not offered, for the new session knows nothing of that
//! agent's transcript to cut ([`Chat::rewind_blocked_by_switch`]).

use gpui_kit::{Context, Window};
use leon_core::AgentId;

use super::chat::{Chat, Handoff};
use super::live::LiveId;
use super::shell::Shell;
use crate::chat::handoff;
use crate::chat::model::Item;
use crate::engine::{MachineState, StatusKind};
use crate::launch::{self, Launch};

impl Chat {
    /// Whether there is a switch of agents after the item at `at`: what is
    /// there was said to the first agent, whose session is let go.
    pub fn rewind_blocked_by_switch(&self, at: usize) -> bool {
        self.conversation
            .items
            .iter()
            .skip(at + 1)
            .any(|item| matches!(item, Item::Switched { .. }))
    }

    /// Whether it can go on with another agent now: there is one to go on
    /// with, and nothing is going on.
    pub fn can_switch(&self) -> bool {
        !self.switchable.is_empty()
            && !self.conversation.running()
            && !self.launching
            && self.rewinding.is_none()
    }

    /// The agents it can go on with, as last looked up.
    pub fn agents_to_switch_to(&self) -> &[AgentId] {
        &self.switchable
    }
}

impl Shell {
    /// Looks up the agents `chat` can go on with: those installed on its
    /// machine that the protocol drives, but the one it has.
    fn agents_for(&self, chat: &Chat) -> Vec<AgentId> {
        self.installed_agents()
            .into_iter()
            .find(|(machine, _)| *machine == chat.machine)
            .map(|(_, agents)| agents)
            .unwrap_or_default()
            .into_iter()
            .filter(|agent| *agent != chat.agent && crate::chat::adapter_of(*agent).is_some())
            .collect()
    }

    /// Looks up the agents a conversation can go on with again.
    pub(super) fn chat_look_for_agents(&mut self, id: LiveId) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        let agents = self.agents_for(chat);
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.switchable = agents;
        }
    }

    /// Goes on with `to` in a conversation. Says whether it did: it does
    /// not while something goes on, nor for an agent that cannot be
    /// started (the status line says why).
    pub(super) fn chat_switch_agent(
        &mut self,
        id: LiveId,
        to: AgentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(chat) = self.chats.get(&id) else {
            return false;
        };
        if chat.conversation.running()
            || chat.launching
            || chat.rewinding.is_some()
            || chat.agent == to
            || crate::chat::adapter_of(to).is_none()
            || chat.turn_is_open()
        {
            return false;
        }
        let Some(machine) = self.snapshot.machine(&chat.machine).cloned() else {
            return false;
        };
        // What starting a conversation with it would be refused for, this is
        // too: it is not installed here, or the folder is gone.
        let report = match self.engine.machine_state(&machine.id) {
            MachineState::Online(Some(report)) => Some(report),
            _ => None,
        };
        let asked = Launch::Agent {
            kind: to,
            resume: None,
            account: None,
        };
        if let Err(error) = launch::plan_with(
            &machine,
            report.as_ref(),
            &chat.cwd,
            &asked,
            &self.engine.ssh(),
            &*self.options.system,
            &Self::launch_prefs(cx),
        ) {
            self.engine.report(StatusKind::Error, error.to_string());
            cx.notify();
            return false;
        }
        let from = chat.agent;
        let block = handoff::text(&chat.conversation.items, from, to, &chat.cwd, handoff::CAP);
        let Some(chat) = self.chats.get_mut(&id) else {
            return false;
        };
        // Nothing was said: there is nothing to hand over and nothing to
        // mark, it only has another agent.
        if block.is_some() {
            chat.conversation.items.push(Item::Switched { from, to });
        }
        chat.handoff = block.map(|text| Handoff {
            text,
            sent_in: None,
        });
        chat.agent = to;
        // The accounts are each agent's own, and so are the session, the
        // model and the modes; what the old one offered is not the new one's.
        chat.account = None;
        chat.history = None;
        chat.conversation.session_id = None;
        chat.conversation.model = None;
        chat.conversation.mode = None;
        chat.conversation.context = None;
        chat.conversation.last_tokens = None;
        chat.mode = None;
        chat.model = None;
        chat.effort = None;
        chat.options = None;
        chat.commands.clear();
        chat.probe_stop();
        chat.menu = None;
        chat.seen = true;
        chat.scroll.scroll_to_bottom();
        chat.input.update(cx, |input, cx| {
            input.set_placeholder(
                format!(
                    "Ask {} anything  \u{b7}  / for commands  \u{b7}  @ for files",
                    crate::format::agent_name(to)
                ),
                window,
                cx,
            );
            input.focus(window, cx);
        });
        self.chat_probe(id, cx);
        self.chat_look_for_agents(id);
        self.rebuild_rows();
        self.refresh_live();
        cx.notify();
        true
    }
}
