//! The current picture of one session, reduced from its beats.
//!
//! A [`Pulse`] answers what a view needs to draw an agent at work: which
//! tools are running, which sub-agents are alive, whether the turn is over.
//! It holds facts only. In particular it knows that a tool call has no
//! result yet, not why: a tool that runs for a minute and a tool waiting for
//! the user's permission look the same in a transcript, and telling them
//! apart needs what the terminal shows, which is the caller's to add.
//!
//! [`Pulse::apply`] is a pure function of the beats applied so far; no clock
//! and no I/O.

use super::beat::{Beat, ToolKind};

/// A tool call that has started and has no result yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInFlight {
    /// The call's id.
    pub id: String,
    /// The tool's name as the agent wrote it.
    pub name: String,
    /// What the tool does.
    pub kind: ToolKind,
    /// The short subject of the call; may be empty.
    pub detail: String,
    /// The whole subject, when the transcript told more than the caption
    /// holds ([`Beat::Brief`]): the command, the path, the question.
    pub brief: Option<String>,
    /// The answers a question offers, by their labels.
    pub options: Vec<String>,
}

/// A sub-agent that was started and has not ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgent {
    /// The id of the tool call that started it.
    pub tool: String,
    /// The sub-agent's own id, once the transcript told it: the one its
    /// transcript file is named after. A sub-agent running inside its call
    /// only tells it when it ends.
    pub agent: Option<String>,
    /// Its type and task, as the call described them; may be empty.
    pub label: String,
    /// It runs in the background: its call returned and the session went
    /// on, or even ended its turn, without it.
    pub background: bool,
    /// The tool the sub-agent itself is running, when its own transcript
    /// is followed too (see [`Pulse::apply_to_subagent`]).
    pub current: Option<ToolInFlight>,
}

/// What the agent is doing, in one word, for a view that shows one thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// The turn is over, or nothing was read yet: the agent waits for the
    /// user.
    Idle,
    /// A message arrived and the model has not written anything yet.
    Prompted,
    /// The model's last word was reasoning.
    Thinking,
    /// The model's last word was text for the user.
    Speaking,
    /// A tool call has no result yet; with several, the newest.
    Working(ToolKind),
    /// A tool result arrived and the model is deciding what to do next.
    Deciding,
}

/// The state of one session, as told by its transcript so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulse {
    tools: Vec<ToolInFlight>,
    subagents: Vec<SubAgent>,
    tool_calls: u64,
    failed_calls: u64,
    compactions: u64,
    last: Option<Beat>,
    turn_over: bool,
    /// What the model did last, when no tool is running.
    resting: Phase,
    context: Option<u64>,
    window: Option<u64>,
}

impl Default for Pulse {
    fn default() -> Self {
        Self::new()
    }
}

impl Pulse {
    /// The picture before any beat: nothing running, the turn over.
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            subagents: Vec::new(),
            tool_calls: 0,
            failed_calls: 0,
            compactions: 0,
            last: None,
            turn_over: true,
            resting: Phase::Idle,
            context: None,
            window: None,
        }
    }

    /// Applies the next beat of the session's own transcript.
    pub fn apply(&mut self, beat: &Beat) {
        if beat.opens_turn() {
            self.turn_over = false;
        }
        match beat {
            // Words and details: they say more of a beat that came before
            // and move nothing, so they are not the last thing that happened.
            Beat::Heard { .. } => return,
            Beat::Brief { id, text, options } => {
                if let Some(tool) = self.tools.iter_mut().find(|tool| tool.id == *id) {
                    tool.brief = Some(text.clone());
                    tool.options = options.clone();
                }
                return;
            }
            Beat::Prompt | Beat::Woken => self.resting = Phase::Prompted,
            Beat::Thinking => self.resting = Phase::Thinking,
            Beat::Said { .. } => self.resting = Phase::Speaking,
            Beat::ToolStarted {
                id,
                name,
                kind,
                detail,
            } => {
                self.tool_calls += 1;
                self.tools.retain(|tool| tool.id != *id);
                self.tools.push(ToolInFlight {
                    id: id.clone(),
                    name: name.clone(),
                    kind: *kind,
                    detail: detail.clone(),
                    brief: None,
                    options: Vec::new(),
                });
                if *kind == ToolKind::Delegate {
                    self.subagents.retain(|agent| agent.tool != *id);
                    self.subagents.push(SubAgent {
                        tool: id.clone(),
                        agent: None,
                        label: detail.clone(),
                        background: false,
                        current: None,
                    });
                }
            }
            Beat::ToolFinished { id, failed, .. } => {
                if *failed {
                    self.failed_calls += 1;
                }
                self.tools.retain(|tool| tool.id != *id);
                // A sub-agent that ran inside its call ends with it.
                self.subagents.retain(|agent| agent.tool != *id);
                self.resting = Phase::Deciding;
            }
            Beat::Detached { id, task } => {
                self.tools.retain(|tool| tool.id != *id);
                match self.subagents.iter_mut().find(|agent| agent.tool == *id) {
                    Some(agent) => {
                        agent.agent = Some(task.clone());
                        agent.background = true;
                    }
                    // The call was written before this reader joined.
                    None => self.subagents.push(SubAgent {
                        tool: id.clone(),
                        agent: Some(task.clone()),
                        label: String::new(),
                        background: true,
                        current: None,
                    }),
                }
                self.resting = Phase::Deciding;
            }
            Beat::TaskEnded { task, tool, .. } => self.subagents.retain(|agent| {
                agent.agent.as_deref() != Some(task.as_str())
                    && tool.as_deref() != Some(agent.tool.as_str())
            }),
            Beat::TasksKilled => self.subagents.retain(|agent| !agent.background),
            Beat::TurnEnded | Beat::Interrupted => {
                self.turn_over = true;
                self.resting = Phase::Idle;
                // Nothing of the turn outlives it except what was sent to
                // the background.
                self.tools.clear();
                self.subagents.retain(|agent| agent.background);
            }
            Beat::Compacted { after, .. } => {
                self.compactions += 1;
                if after.is_some() {
                    self.context = *after;
                }
            }
            Beat::Usage {
                context, window, ..
            } => {
                self.context = Some(*context);
                if window.is_some() {
                    self.window = *window;
                }
            }
        }
        self.last = Some(beat.clone());
    }

    /// Applies a beat of a sub-agent's own transcript: it updates the tool
    /// that sub-agent is running and nothing else. `agent` is the
    /// sub-agent's id or the id of the tool call that started it. Returns
    /// whether such a sub-agent is alive.
    pub fn apply_to_subagent(&mut self, agent: &str, beat: &Beat) -> bool {
        let Some(found) = self
            .subagents
            .iter_mut()
            .find(|one| one.agent.as_deref() == Some(agent) || one.tool == agent)
        else {
            return false;
        };
        match beat {
            Beat::ToolStarted {
                id,
                name,
                kind,
                detail,
            } => {
                found.current = Some(ToolInFlight {
                    id: id.clone(),
                    name: name.clone(),
                    kind: *kind,
                    detail: detail.clone(),
                    brief: None,
                    options: Vec::new(),
                });
            }
            Beat::Brief { id, text, options } => {
                if let Some(tool) = found.current.as_mut().filter(|tool| tool.id == *id) {
                    tool.brief = Some(text.clone());
                    tool.options = options.clone();
                }
            }
            Beat::ToolFinished { id, .. } | Beat::Detached { id, .. } => {
                if found.current.as_ref().is_some_and(|tool| tool.id == *id) {
                    found.current = None;
                }
            }
            Beat::TurnEnded | Beat::Interrupted => found.current = None,
            _ => {}
        }
        true
    }

    /// Tells the picture which sub-agent a tool call started, when that is
    /// learned from outside the transcript (the sub-agent's `meta.json`
    /// names its call). Returns whether such a call is alive.
    pub fn name_subagent(&mut self, tool: &str, agent: &str) -> bool {
        match self.subagents.iter_mut().find(|one| one.tool == tool) {
            Some(found) => {
                found.agent = Some(agent.to_owned());
                true
            }
            None => false,
        }
    }

    /// The tool calls without a result, oldest first.
    pub fn tools(&self) -> &[ToolInFlight] {
        &self.tools
    }

    /// Whether a tool call has no result yet. True while a tool runs and
    /// while a permission question about it waits for the user: the
    /// transcript cannot tell the two apart.
    pub fn awaits_result(&self) -> bool {
        !self.tools.is_empty()
    }

    /// The sub-agents alive, oldest first.
    pub fn subagents(&self) -> &[SubAgent] {
        &self.subagents
    }

    /// How many tool calls the session made, as far as it was read.
    pub fn tool_calls(&self) -> u64 {
        self.tool_calls
    }

    /// How many of them ended in an error.
    pub fn failed_calls(&self) -> u64 {
        self.failed_calls
    }

    /// How many times the conversation was compacted.
    pub fn compactions(&self) -> u64 {
        self.compactions
    }

    /// The last beat applied.
    pub fn last(&self) -> Option<&Beat> {
        self.last.as_ref()
    }

    /// Whether the agent finished its turn and waits for the user. Also
    /// true before any beat. Background sub-agents may still be alive.
    pub fn turn_over(&self) -> bool {
        self.turn_over
    }

    /// The size of the context at the last model call, in tokens.
    pub fn context_tokens(&self) -> Option<u64> {
        self.context
    }

    /// The size of the model's context window, when the transcript says.
    pub fn context_window(&self) -> Option<u64> {
        self.window
    }

    /// What the agent is doing, in one word.
    pub fn phase(&self) -> Phase {
        if self.turn_over {
            return Phase::Idle;
        }
        match self.tools.last() {
            Some(tool) => Phase::Working(tool.kind),
            None => self.resting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::beat::TaskOutcome;

    fn started(id: &str, name: &str, detail: &str) -> Beat {
        Beat::ToolStarted {
            id: id.into(),
            name: name.into(),
            kind: ToolKind::of(name),
            detail: detail.into(),
        }
    }

    fn finished(id: &str, failed: bool) -> Beat {
        Beat::ToolFinished {
            id: id.into(),
            failed,
            refused: false,
        }
    }

    fn pulse_of(beats: &[Beat]) -> Pulse {
        let mut pulse = Pulse::new();
        for beat in beats {
            pulse.apply(beat);
        }
        pulse
    }

    #[test]
    fn a_new_pulse_is_idle_with_nothing_running() {
        let pulse = Pulse::new();
        assert!(pulse.turn_over());
        assert!(!pulse.awaits_result());
        assert_eq!(pulse.phase(), Phase::Idle);
        assert_eq!(pulse.last(), None);
        assert_eq!(pulse.tool_calls(), 0);
        assert_eq!(pulse, Pulse::default());
    }

    #[test]
    fn the_phase_follows_the_turn() {
        let mut pulse = Pulse::new();
        let mut phases = Vec::new();
        for beat in [
            Beat::Prompt,
            Beat::Thinking,
            Beat::said("ok"),
            started("t1", "Bash", "cargo test"),
            finished("t1", false),
            Beat::said("ok"),
            Beat::TurnEnded,
        ] {
            pulse.apply(&beat);
            phases.push(pulse.phase());
        }
        assert_eq!(
            phases,
            [
                Phase::Prompted,
                Phase::Thinking,
                Phase::Speaking,
                Phase::Working(ToolKind::Run),
                Phase::Deciding,
                Phase::Speaking,
                Phase::Idle
            ]
        );
        assert!(pulse.turn_over());
        assert_eq!(pulse.last(), Some(&Beat::TurnEnded));
    }

    #[test]
    fn tools_in_flight_are_kept_until_their_result() {
        let mut pulse = pulse_of(&[
            Beat::Prompt,
            started("t1", "Read", "main.rs"),
            started("t2", "Grep", "fn main"),
        ]);
        assert!(pulse.awaits_result());
        assert_eq!(pulse.phase(), Phase::Working(ToolKind::Search));
        let ids: Vec<_> = pulse.tools().iter().map(|tool| tool.id.as_str()).collect();
        assert_eq!(ids, ["t1", "t2"]);
        assert_eq!(pulse.tools()[0].detail, "main.rs");

        pulse.apply(&finished("t2", true));
        assert_eq!(pulse.phase(), Phase::Working(ToolKind::Read));
        pulse.apply(&finished("t1", false));
        assert!(!pulse.awaits_result());
        assert_eq!(pulse.phase(), Phase::Deciding);
        assert_eq!((pulse.tool_calls(), pulse.failed_calls()), (2, 1));
    }

    #[test]
    fn a_result_for_an_unknown_call_changes_nothing_running() {
        let mut pulse = pulse_of(&[Beat::Prompt, started("t1", "Bash", "ls")]);
        pulse.apply(&finished("some-other-call", false));
        assert_eq!(pulse.tools().len(), 1);
    }

    #[test]
    fn usage_does_not_hide_what_the_model_is_doing() {
        let pulse = pulse_of(&[
            Beat::Prompt,
            Beat::Thinking,
            Beat::Usage {
                context: 1000,
                output: 5,
                window: Some(200_000),
            },
        ]);
        assert_eq!(pulse.phase(), Phase::Thinking);
        assert!(!pulse.turn_over());
        assert_eq!(pulse.context_tokens(), Some(1000));
        assert_eq!(pulse.context_window(), Some(200_000));
        assert!(matches!(pulse.last(), Some(Beat::Usage { .. })));
    }

    #[test]
    fn a_sub_agent_inside_its_call_lives_as_long_as_the_call() {
        let mut pulse = pulse_of(&[Beat::Prompt, started("t1", "Agent", "Explore: Map it")]);
        assert_eq!(pulse.phase(), Phase::Working(ToolKind::Delegate));
        assert_eq!(
            pulse.subagents(),
            [SubAgent {
                tool: "t1".into(),
                agent: None,
                label: "Explore: Map it".into(),
                background: false,
                current: None
            }]
        );
        pulse.apply(&finished("t1", false));
        assert_eq!(pulse.subagents(), []);
    }

    #[test]
    fn a_background_sub_agent_outlives_its_call_and_the_turn() {
        let mut pulse = pulse_of(&[
            Beat::Prompt,
            started("t1", "Agent", "Explore: Map it"),
            Beat::Detached {
                id: "t1".into(),
                task: "a1".into(),
            },
        ]);
        assert!(!pulse.awaits_result(), "the call itself returned");
        assert_eq!(pulse.subagents()[0].agent.as_deref(), Some("a1"));
        assert!(pulse.subagents()[0].background);

        pulse.apply(&Beat::said("ok"));
        pulse.apply(&Beat::TurnEnded);
        assert!(pulse.turn_over());
        assert_eq!(pulse.subagents().len(), 1);

        pulse.apply(&Beat::TaskEnded {
            task: "a1".into(),
            tool: Some("t1".into()),
            outcome: TaskOutcome::Completed,
        });
        assert_eq!(pulse.subagents(), []);
        assert!(pulse.turn_over(), "the notice alone does not reopen a turn");
        pulse.apply(&Beat::Woken);
        assert_eq!(pulse.phase(), Phase::Prompted);
    }

    #[test]
    fn a_task_end_matches_by_task_or_by_call() {
        let two = [
            Beat::Prompt,
            started("t1", "Agent", "one"),
            Beat::Detached {
                id: "t1".into(),
                task: "a1".into(),
            },
            started("t2", "Agent", "two"),
            Beat::Detached {
                id: "t2".into(),
                task: "a2".into(),
            },
        ];
        let mut pulse = pulse_of(&two);
        pulse.apply(&Beat::TaskEnded {
            task: "a1".into(),
            tool: None,
            outcome: TaskOutcome::Failed,
        });
        assert_eq!(pulse.subagents().len(), 1);
        pulse.apply(&Beat::TaskEnded {
            task: "unknown".into(),
            tool: Some("t2".into()),
            outcome: TaskOutcome::Stopped,
        });
        assert_eq!(pulse.subagents(), []);

        let mut killed = pulse_of(&two);
        killed.apply(&started("t3", "Agent", "inside its call"));
        killed.apply(&Beat::TasksKilled);
        assert_eq!(killed.subagents().len(), 1);
        assert_eq!(killed.subagents()[0].tool, "t3");
    }

    #[test]
    fn a_detached_call_seen_without_its_start_is_still_a_sub_agent() {
        let pulse = pulse_of(&[Beat::Detached {
            id: "t1".into(),
            task: "a1".into(),
        }]);
        assert_eq!(pulse.subagents().len(), 1);
        assert_eq!(pulse.subagents()[0].label, "");
        assert!(pulse.turn_over());
    }

    #[test]
    fn an_interruption_ends_the_turn_and_what_was_running_in_it() {
        let pulse = pulse_of(&[
            Beat::Prompt,
            started("t1", "Bash", "sleep 600"),
            started("t2", "Agent", "inside its call"),
            started("t3", "Agent", "background"),
            Beat::Detached {
                id: "t3".into(),
                task: "a3".into(),
            },
            Beat::Interrupted,
        ]);
        assert!(pulse.turn_over());
        assert!(!pulse.awaits_result());
        assert_eq!(pulse.phase(), Phase::Idle);
        assert_eq!(pulse.subagents().len(), 1);
        assert_eq!(pulse.subagents()[0].tool, "t3");
    }

    #[test]
    fn a_sub_agents_own_tool_is_shown_on_it() {
        let mut pulse = pulse_of(&[
            Beat::Prompt,
            started("t1", "Agent", "Explore: Map it"),
            Beat::Detached {
                id: "t1".into(),
                task: "a1".into(),
            },
        ]);
        let calls = pulse.tool_calls();
        assert!(pulse.apply_to_subagent("a1", &started("s1", "Grep", "fn parse")));
        assert_eq!(
            pulse.subagents()[0]
                .current
                .as_ref()
                .map(|t| t.detail.as_str()),
            Some("fn parse")
        );
        assert_eq!(pulse.tool_calls(), calls, "a sub-agent's calls are its own");
        assert!(pulse.tools().is_empty());

        assert!(pulse.apply_to_subagent("a1", &finished("another", false)));
        assert!(pulse.subagents()[0].current.is_some());
        assert!(pulse.apply_to_subagent("t1", &finished("s1", false)));
        assert_eq!(pulse.subagents()[0].current, None);

        assert!(!pulse.apply_to_subagent("nobody", &Beat::Thinking));
    }

    #[test]
    fn a_sub_agent_can_be_named_from_outside_the_transcript() {
        let mut pulse = pulse_of(&[Beat::Prompt, started("t1", "Task", "Map it")]);
        assert!(pulse.name_subagent("t1", "a1"));
        assert!(!pulse.name_subagent("t2", "a2"));
        assert!(pulse.apply_to_subagent("a1", &started("s1", "Read", "lib.rs")));
        assert_eq!(pulse.subagents()[0].agent.as_deref(), Some("a1"));
    }

    #[test]
    fn a_compaction_is_counted_and_updates_the_context_size() {
        let pulse = pulse_of(&[
            Beat::Usage {
                context: 180_000,
                output: 10,
                window: None,
            },
            Beat::Compacted {
                before: Some(180_000),
                after: Some(21_000),
            },
            Beat::Compacted {
                before: None,
                after: None,
            },
        ]);
        assert_eq!(pulse.compactions(), 2);
        assert_eq!(pulse.context_tokens(), Some(21_000));
    }

    #[test]
    fn the_same_call_started_twice_is_one_call_in_flight() {
        let pulse = pulse_of(&[started("t1", "Agent", "one"), started("t1", "Agent", "one")]);
        assert_eq!(pulse.tools().len(), 1);
        assert_eq!(pulse.subagents().len(), 1);
    }

    #[test]
    fn a_brief_is_kept_with_its_call_and_the_words_of_a_prompt_move_nothing() {
        let brief = |id: &str| Beat::Brief {
            id: id.into(),
            text: "Which crate first?".into(),
            options: vec!["api".into(), "web".into()],
        };
        let mut pulse = pulse_of(&[
            Beat::Prompt,
            Beat::Heard {
                text: "ask me".into(),
                at: None,
            },
        ]);
        // The words of the prompt are not what happened last, and open
        // nothing a prompt did not open.
        assert_eq!(pulse.last(), Some(&Beat::Prompt));
        assert_eq!(pulse.phase(), Phase::Prompted);
        pulse.apply(&started("t1", "AskUserQuestion", "Scope"));
        pulse.apply(&brief("t1"));
        let tool = &pulse.tools()[0];
        assert_eq!(tool.brief.as_deref(), Some("Which crate first?"));
        assert_eq!(tool.options, ["api", "web"]);
        assert!(matches!(pulse.last(), Some(Beat::ToolStarted { .. })));
        // A brief of a call that is not in flight is dropped.
        pulse.apply(&brief("t9"));
        assert_eq!(pulse.tools().len(), 1);
        // After a turn ended, words alone do not open another.
        let mut over = pulse_of(&[Beat::Prompt, Beat::TurnEnded]);
        over.apply(&Beat::Heard {
            text: "late".into(),
            at: None,
        });
        assert!(over.turn_over());
        // A sub-agent's own call keeps its brief too.
        let mut parent = pulse_of(&[Beat::Prompt, started("a1", "Agent", "Explore: Map it")]);
        parent.apply_to_subagent("a1", &started("s1", "Bash", "ls"));
        parent.apply_to_subagent(
            "a1",
            &Beat::Brief {
                id: "s1".into(),
                text: "ls -la /srv".into(),
                options: Vec::new(),
            },
        );
        let current = parent.subagents()[0].current.as_ref().unwrap();
        assert_eq!(current.brief.as_deref(), Some("ls -la /srv"));
    }
}
