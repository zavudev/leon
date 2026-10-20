//! The events of a turn, as they cross to the application.
//!
//! These are the types of Leon's unified agent protocol (`leon-turn`), which
//! reads each agent's own lines and reduces them to the few events a chat
//! shows. They are declared again here, field for field, because what crosses
//! a language boundary is exported from this crate; the conversions at the
//! end of the file are the whole difference.

/// What happens during a turn, as a chat shows it.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum TurnEvent {
    /// The agent's conversation exists. `session_id` is what a later turn
    /// passes as `resume_id` to continue it.
    Started {
        /// The id the agent gave the conversation.
        session_id: String,
        /// The model answering, when the agent says.
        model: Option<String>,
    },
    /// More of the message being written. Deltas accumulate into a draft
    /// that the next [`TurnEvent::Text`] replaces.
    TextDelta {
        /// The next characters.
        text: String,
    },
    /// A complete message of the agent. It replaces the draft built from the
    /// deltas before it, if any.
    Text {
        /// The whole message.
        text: String,
    },
    /// More of what the agent thinks before it answers, when it says. Not
    /// part of the answer: a chat keeps it apart, and out of sight unless
    /// asked for.
    Thought {
        /// The next characters.
        text: String,
    },
    /// The agent started using a tool.
    ToolUse {
        /// Pairs it with its [`TurnEvent::ToolResult`].
        id: String,
        /// The tool (`Bash`, `Read`, `shell`…).
        name: String,
        /// One line saying what with: the command, the file, the query.
        summary: String,
        /// What the line leaves out: the whole command, the change of a file
        /// as removed (`- `) and added (`+ `) lines under its path, or the
        /// arguments one per line. Empty when the summary says it all.
        detail: String,
    },
    /// A tool finished.
    ToolResult {
        /// The id of its [`TurnEvent::ToolUse`].
        id: String,
        /// Whether it worked.
        ok: bool,
        /// One line of what came back.
        summary: String,
        /// Everything that came back, as text: the output, the listing, the
        /// error. Empty when the summary says it all.
        detail: String,
    },
    /// The tokens the turn used so far, as the agent counts them: each one
    /// replaces the one before, and the last is the turn's.
    Usage {
        /// Read by the model, cached reads included.
        input: u64,
        /// Of those, read from the provider's cache.
        cached_input: u64,
        /// Written by the model, its reasoning included.
        output: u64,
        /// Of those, its reasoning, when the agent says.
        reasoning: u64,
        /// How many the model can hold at once, when the agent says.
        context_window: Option<u64>,
    },
    /// The agent finished the turn. Exactly one per turn, before
    /// [`TurnEvent::Exited`].
    Finished {
        /// Whether the agent says it succeeded.
        ok: bool,
        /// What went wrong, when it did not.
        error: Option<String>,
        /// What the turn cost, when the agent says.
        cost_usd: Option<f64>,
        /// How long it took, when the agent says.
        duration_ms: Option<u64>,
        /// Tools the agent wanted and the permission mode refused.
        denied_tools: Vec<String>,
        /// The person stopped it ([`crate::Turn::interrupt`]); `ok` is then
        /// false without anything having gone wrong.
        interrupted: bool,
    },
    /// The agent asks something and waits for [`crate::Turn::answer`].
    /// Several may be open at once.
    Question {
        /// Names the question in the answer and in
        /// [`TurnEvent::QuestionResolved`].
        id: String,
        /// What is asked.
        kind: QuestionKind,
    },
    /// The question is no longer open: its answer reached the agent, or the
    /// agent withdrew it, or the turn went on without it.
    QuestionResolved {
        /// The id of its [`TurnEvent::Question`].
        id: String,
    },
    /// The agent works in another permission mode now: the person changed
    /// it, or an answer did (approving a plan, "always allow").
    ModeChanged {
        /// The mode, as the agent names it.
        mode: String,
    },
    /// The connection dropped; the turn keeps running on the other computer.
    Disconnected,
    /// The connection is back and the turn was attached again.
    Reconnected,
    /// Part of the output was missed and can no longer be had (the host
    /// keeps the last 2 MiB). What follows is the turn from somewhere in the
    /// middle.
    Gap,
    /// The agent's process ended. The last event of a turn.
    Exited {
        /// Its exit code.
        code: u32,
    },
    /// The turn no longer exists on the other computer. The last event.
    Gone,
}

/// What an agent asks the person.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum QuestionKind {
    /// May it use this tool, this way?
    Permission {
        /// The tool (`Bash`, `Edit`…).
        tool: String,
        /// One line saying what with: the command, the file.
        summary: String,
        /// Everything it would pass to the tool, as text, for whoever wants
        /// to read it before answering.
        detail: String,
        /// Why it has to ask, when the agent says.
        reason: Option<String>,
        /// What may be answered; one of them goes back in
        /// [`Answer::Permission`].
        choices: Vec<PermissionChoice>,
    },
    /// One or more questions with options to pick from.
    Choices {
        /// The questions, to be answered together.
        questions: Vec<ChoiceQuestion>,
    },
    /// It planned and asks to start working.
    Plan {
        /// The plan, as Markdown.
        plan: String,
    },
}

/// What a way of answering a permission question does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PermissionEffect {
    /// Allows this one use.
    AllowOnce,
    /// Allows it and changes a rule, so the like is not asked again.
    AllowAlways,
    /// Refuses it. A reason may be given with the answer.
    Deny,
}

/// One way of answering a permission question.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PermissionChoice {
    /// What goes back in [`Answer::Permission`].
    pub id: String,
    /// What it does.
    pub effect: PermissionEffect,
    /// Words for it.
    pub label: String,
    /// How far an "always" reaches (this session, this project, everywhere),
    /// when the agent says.
    pub scope: Option<String>,
    /// Whether the agent reads a reason given with it ([`Answer::Permission`]).
    /// Without it, what to do instead is said as the next message.
    pub takes_reason: bool,
}

/// One question with options.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChoiceQuestion {
    /// Names the question in [`ChoiceAnswer`].
    pub id: String,
    /// A word or two saying what it is about, when the agent gives one.
    pub header: Option<String>,
    /// The question.
    pub question: String,
    /// What can be picked.
    pub options: Vec<ChoiceOption>,
    /// Whether several may be picked.
    pub multi_select: bool,
    /// Whether an answer in the person's own words is accepted.
    pub allow_other: bool,
}

/// One thing that can be picked.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChoiceOption {
    /// What is picked; it goes back in [`ChoiceAnswer`].
    pub label: String,
    /// What picking it means, when the agent says.
    pub description: Option<String>,
}

/// The answer to one [`ChoiceQuestion`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChoiceAnswer {
    /// The question's id.
    pub question_id: String,
    /// The labels picked.
    pub labels: Vec<String>,
    /// An answer in the person's own words, with or instead of labels.
    pub other: Option<String>,
}

/// What the person answers to a [`TurnEvent::Question`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Answer {
    /// To [`QuestionKind::Permission`].
    Permission {
        /// The id of the [`PermissionChoice`] taken.
        choice_id: String,
        /// Why not, with a refusal; the agent reads it.
        reason: Option<String>,
    },
    /// To [`QuestionKind::Choices`]: one answer per question.
    Choices {
        /// The answers.
        answers: Vec<ChoiceAnswer>,
    },
    /// To [`QuestionKind::Plan`].
    Plan {
        /// Start working, or keep planning.
        approve: bool,
        /// What to change, when it should keep planning.
        feedback: Option<String>,
    },
}

impl TurnEvent {
    /// The event as it crosses to the application, or nothing for one the
    /// application does not show.
    pub(crate) fn crossing(event: leon_turn::TurnEvent) -> Option<Self> {
        use leon_turn::TurnEvent as T;
        Some(match event {
            T::Usage { tokens } => Self::Usage {
                input: tokens.input,
                cached_input: tokens.cached_input,
                output: tokens.output,
                reasoning: tokens.reasoning,
                context_window: tokens.context_window,
            },
            T::Started { session_id, model } => Self::Started { session_id, model },
            T::TextDelta { text } => Self::TextDelta { text },
            T::Text { text } => Self::Text { text },
            T::Thought { text } => Self::Thought { text },
            T::ToolUse {
                id,
                name,
                summary,
                detail,
            } => Self::ToolUse {
                id,
                name,
                summary,
                detail,
            },
            T::ToolResult {
                id,
                ok,
                summary,
                detail,
            } => Self::ToolResult {
                id,
                ok,
                summary,
                detail,
            },
            T::Finished {
                ok,
                error,
                cost_usd,
                duration_ms,
                denied_tools,
                interrupted,
            } => Self::Finished {
                ok,
                error,
                cost_usd,
                duration_ms,
                denied_tools,
                interrupted,
            },
            T::Question { id, kind } => Self::Question {
                id,
                kind: kind.into(),
            },
            T::QuestionResolved { id } => Self::QuestionResolved { id },
            T::ModeChanged { mode } => Self::ModeChanged { mode },
            T::Disconnected => Self::Disconnected,
            T::Reconnected => Self::Reconnected,
            T::Gap => Self::Gap,
            T::Exited { code } => Self::Exited { code },
            T::Gone => Self::Gone,
        })
    }
}

impl From<leon_turn::QuestionKind> for QuestionKind {
    fn from(kind: leon_turn::QuestionKind) -> Self {
        use leon_turn::QuestionKind as K;
        match kind {
            K::Permission {
                tool,
                summary,
                detail,
                reason,
                choices,
            } => Self::Permission {
                tool,
                summary,
                detail,
                reason,
                choices: choices.into_iter().map(Into::into).collect(),
            },
            K::Choices { questions } => Self::Choices {
                questions: questions.into_iter().map(Into::into).collect(),
            },
            K::Plan { plan } => Self::Plan { plan },
        }
    }
}

impl From<leon_turn::PermissionEffect> for PermissionEffect {
    fn from(effect: leon_turn::PermissionEffect) -> Self {
        match effect {
            leon_turn::PermissionEffect::AllowOnce => Self::AllowOnce,
            leon_turn::PermissionEffect::AllowAlways => Self::AllowAlways,
            leon_turn::PermissionEffect::Deny => Self::Deny,
        }
    }
}

impl From<leon_turn::PermissionChoice> for PermissionChoice {
    fn from(choice: leon_turn::PermissionChoice) -> Self {
        Self {
            id: choice.id,
            effect: choice.effect.into(),
            label: choice.label,
            scope: choice.scope,
            takes_reason: choice.takes_reason,
        }
    }
}

impl From<leon_turn::ChoiceQuestion> for ChoiceQuestion {
    fn from(question: leon_turn::ChoiceQuestion) -> Self {
        Self {
            id: question.id,
            header: question.header,
            question: question.question,
            options: question.options.into_iter().map(Into::into).collect(),
            multi_select: question.multi_select,
            allow_other: question.allow_other,
        }
    }
}

impl From<leon_turn::ChoiceOption> for ChoiceOption {
    fn from(option: leon_turn::ChoiceOption) -> Self {
        Self {
            label: option.label,
            description: option.description,
        }
    }
}

impl From<ChoiceAnswer> for leon_turn::ChoiceAnswer {
    fn from(answer: ChoiceAnswer) -> Self {
        Self {
            question_id: answer.question_id,
            labels: answer.labels,
            other: answer.other,
        }
    }
}

impl From<Answer> for leon_turn::Answer {
    fn from(answer: Answer) -> Self {
        match answer {
            Answer::Permission { choice_id, reason } => Self::Permission { choice_id, reason },
            Answer::Choices { answers } => Self::Choices {
                answers: answers.into_iter().map(Into::into).collect(),
            },
            Answer::Plan { approve, feedback } => Self::Plan { approve, feedback },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turns_tokens_cross_to_the_application() {
        let tokens = leon_turn::Tokens {
            input: 45_100,
            cached_input: 31_000,
            output: 3_100,
            reasoning: 900,
            context_window: Some(200_000),
        };
        assert_eq!(
            TurnEvent::crossing(leon_turn::TurnEvent::Usage { tokens }),
            Some(TurnEvent::Usage {
                input: 45_100,
                cached_input: 31_000,
                output: 3_100,
                reasoning: 900,
                context_window: Some(200_000),
            })
        );
    }
}
