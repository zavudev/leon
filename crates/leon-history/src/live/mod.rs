//! A live reading of an agent's transcript: what the agent is doing now.
//!
//! The history import reads a finished transcript once and keeps what was
//! said. This module follows a transcript **while it grows** and keeps what
//! is happening: which tool runs, on what, whether a sub-agent is alive,
//! whether the turn is over. It is what lets a view show an agent at work
//! from facts instead of guessing from the terminal.
//!
//! The pieces, each usable alone:
//!
//! * [`Tail`] turns appended bytes into [`Beat`]s. It is pure: no file, no
//!   clock. [`read_appended`] is the file read beside it and [`Follower`]
//!   joins the two.
//! * [`Pulse`] reduces beats to the current picture of one session.
//! * [`find_claude_transcript`], [`list_claude_subagents`] and
//!   [`find_codex_rollout`] find the files to follow; the path arithmetic
//!   under them is pure.
//!
//! # Where each beat comes from
//!
//! In a Claude Code transcript (see the `claude` submodule for the format):
//!
//! | Beat | Line |
//! | --- | --- |
//! | [`Beat::Prompt`] | a `user` line with text or a picture, not injected (`isMeta`), not a local command, whose `origin.kind` is `human` or absent |
//! | [`Beat::Heard`] | the user's own words of that line ([`heard`]: without the markers of pictures, a paste as [`PASTED`]), after its [`Beat::Prompt`]; none for a picture alone. Also a slash command typed with words after it (`<command-name>` with a non-empty `<command-args>`), as `/name words`, which is no prompt by itself |
//! | [`Beat::Woken`] | a `user` line whose `origin.kind` is not `human`: a task notification, a message from another agent |
//! | [`Beat::Thinking`] | an `assistant` line with a `thinking` block |
//! | [`Beat::Said`] | an `assistant` line with a non-empty `text` block; it carries the text ([`speech`]: whole but for control characters, at most [`MAX_SPEECH_CHARS`]) and the line's `timestamp` |
//! | [`Beat::ToolStarted`] | an `assistant` line with a `tool_use` block |
//! | [`Beat::Brief`] | the same block, after its [`Beat::ToolStarted`], when its input says more than the caption: the whole command, path or address, the plan, or the question and its answers ([`brief`]) |
//! | [`Beat::ToolFinished`] | a `tool_result` block of a `user` line; `is_error` is `failed` |
//! | [`Beat::Detached`] | a `tool_result` whose line has `toolUseResult.agentId` and an `async_launched` status |
//! | [`Beat::TaskEnded`] | a `<task-notification>` with a `<task-id>` and a `<status>` |
//! | [`Beat::TasksKilled`] | a `system` line of subtype `agents_killed` |
//! | [`Beat::TurnEnded`] | an `assistant` line with text or a tool call whose `stop_reason` is `end_turn`, `stop_sequence` or `refusal`; or a `system` line of subtype `turn_duration`. Told once per turn |
//! | [`Beat::Interrupted`] | a `user` line starting with `[Request interrupted by user` |
//! | [`Beat::Compacted`] | a `system` line of subtype `compact_boundary`, with `compactMetadata` |
//! | [`Beat::Usage`] | `message.usage` of an `assistant` line, when the numbers changed |
//!
//! In a Codex rollout (see the `codex` submodule): `task_started`,
//! `task_complete` and `turn_aborted` events are [`Beat::Prompt`],
//! [`Beat::TurnEnded`] and [`Beat::Interrupted`]; `reasoning` and assistant
//! `message` items are [`Beat::Thinking`] and [`Beat::Said`]; tool calls and
//! their outputs are matched by `call_id`; `token_count` is [`Beat::Usage`]
//! and `compacted` is [`Beat::Compacted`]. A `message` item of the user's
//! role is [`Beat::Heard`], but for the blocks the harness writes there
//! itself (those that open with a tag or with the heading of the folder's
//! instructions); where the interface wrote what was attached first, only
//! what stands under `## My request:` is the user's. A call's [`Beat::Brief`] is read from its arguments as in
//! a Claude Code transcript. Codex has no sub-agent beats.
//!
//! # What a transcript does not say
//!
//! * **A pending permission question.** The tool call is written before the
//!   question is asked and nothing is written until it is answered, so a
//!   call waiting for permission and a call that is running are the same
//!   [`ToolInFlight`]. [`Pulse::awaits_result`] is the fact; the caller adds
//!   what the terminal shows.
//! * **Words as they stream.** A block is written when it is complete, so
//!   [`Beat::Thinking`] and [`Beat::Said`] arrive when the reasoning or the
//!   text is finished, not when it starts.
//! * **A sub-agent's id while it runs inside its call.** Only a background
//!   launch tells the id at once; for the others the sub-agent's
//!   `meta.json` names the call (see [`SubagentMeta`] and
//!   [`Pulse::name_subagent`]).

mod beat;
mod claude;
mod codex;
mod locate;
mod pulse;
mod tail;

pub use beat::{
    brief, detail, heard, speech, Beat, TaskOutcome, ToolKind, MAX_BRIEF_CHARS, MAX_DETAIL_CHARS,
    MAX_OPTIONS, MAX_SPEECH_CHARS, PASTED,
};
pub use locate::{
    claude_project_dir_name, claude_subagent_files, claude_subagent_path, claude_subagents_dir,
    claude_transcript_path, find_claude_transcript, find_codex_rollout, is_codex_rollout_of,
    list_claude_subagents, SubagentFile, SubagentMeta,
};
pub use pulse::{Phase, Pulse, SubAgent, ToolInFlight};
pub use tail::{read_appended, Appended, Follower, Format, Polled, Start, Tail};
