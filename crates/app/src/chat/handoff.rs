//! What a new agent is told of the conversation it takes over.
//!
//! Switching the agent of a chat (Claude Code to Codex, say) starts a new
//! session of the other agent in the same folder, and that session knows
//! nothing of what was said. The way to keep the context is Leon's own
//! transcript ([`Item`]s): [`text`] writes it as a block that goes before
//! the person's next message, in a size an agent takes without noticing
//! (`CAP`):
//!
//! * the person's words and the agent's answers, verbatim, for as many of
//!   the latest turns as fit;
//! * the tools of those turns, one line each (what they did to files, with
//!   the lines that came and went);
//! * the turns before them, one line each, the oldest left out first;
//! * the agent's list of things to do as it stands, and the files the
//!   conversation changed.
//!
//! The block is wrapped in a tag ([`marker`]) that the agent records, with the
//! rest of the message, in its own history: that is how a conversation read
//! back from there shows the switch and not a page of someone else's words
//! (`super::harness`, `super::model::Conversation::resumed`).
//!
//! Pure: items in, words out.

use leon_core::AgentId;
use leon_turn::TodoStatus;

use super::changed;
use super::model::{Item, Tool};
use super::phrase;

/// How long the block is at most, in characters.
pub const CAP: usize = 24_000;

/// The tag that wraps the block, as the harness's list knows it.
pub const TAG: &str = "leon-handoff";

/// How long one message of a recent turn is kept, in characters, from the
/// most to the least: a turn that does not fit with the first is tried with
/// the next.
const MESSAGE_CAPS: [usize; 3] = [6_000, 2_000, 400];

/// How long the line of a tool is.
const TOOL_LINE: usize = 160;

/// How long each half of the line of an older turn is.
const SUMMARY_LINE: usize = 160;

/// What the headings of the two lists of turns take, with room to spare.
const HEADINGS: usize = 160;

/// How many things to do and how many files the block names at most.
const MAX_TODOS: usize = 30;
const MAX_FILES: usize = 40;

/// The opening tag of the block of a switch from `from` to `to`.
pub fn marker(from: AgentId, to: AgentId) -> String {
    format!("<{TAG} from=\"{}\" to=\"{}\">", from.as_str(), to.as_str())
}

/// The agents of the switch a message tells of, when it begins with the
/// block.
pub fn parse_marker(text: &str) -> Option<(AgentId, AgentId)> {
    let rest = text.trim_start().strip_prefix(&format!("<{TAG} from=\""))?;
    let (from, rest) = rest.split_once("\" to=\"")?;
    let (to, _) = rest.split_once("\">")?;
    Some((AgentId::parse(from)?, AgentId::parse(to)?))
}

/// One turn of the conversation: what the person said and what came of it.
struct Turn<'a> {
    items: Vec<&'a Item>,
}

impl Turn<'_> {
    fn user(&self) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            Item::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
    }

    /// The whole turn: each message up to `cap` characters, each tool a line.
    fn whole(&self, cap: usize, root: &str) -> String {
        let mut out = String::new();
        let mut tools = false;
        for item in &self.items {
            match item {
                Item::User { text, .. } => {
                    tools = false;
                    out.push_str(&format!("User:\n{}\n\n", clip(text, cap)));
                }
                Item::Agent { text, .. } if !text.trim().is_empty() => {
                    tools = false;
                    out.push_str(&format!("Assistant:\n{}\n\n", clip(text, cap)));
                }
                Item::Tool(tool) => {
                    if !tools {
                        out.push_str("Tools used:\n");
                        tools = true;
                    }
                    out.push_str(&format!("- {}\n", tool_line(tool, root)));
                }
                _ => {}
            }
        }
        out.trim_end().to_owned() + "\n"
    }

    /// The turn in one line: the person's first words, the agent's, and
    /// what the tools did in words.
    fn line(&self) -> String {
        let first = |text: &str| {
            let line = text.lines().map(str::trim).find(|line| !line.is_empty());
            clip_head(line.unwrap_or_default(), SUMMARY_LINE)
        };
        let mut parts = Vec::new();
        if let Some(user) = self.user() {
            parts.push(format!("User: {}", first(user)));
        }
        let answer = self.items.iter().find_map(|item| match item {
            Item::Agent { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        });
        if let Some(answer) = answer {
            parts.push(format!("Assistant: {}", first(answer)));
        }
        let tools: Vec<&Tool> = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect();
        if !tools.is_empty() {
            parts.push(format!("({})", phrase::summary(&tools)));
        }
        format!("- {}\n", parts.join(" "))
    }
}

/// The conversation cut into turns: each begins with the person's words,
/// and what came before the first of them is a turn of its own.
fn turns(items: &[Item]) -> Vec<Turn<'_>> {
    let mut out: Vec<Turn> = Vec::new();
    for item in items {
        let content = matches!(item, Item::User { .. } | Item::Agent { .. } | Item::Tool(_));
        if !content {
            continue;
        }
        match (item, out.last_mut()) {
            (Item::User { .. }, _) | (_, None) => out.push(Turn { items: vec![item] }),
            (_, Some(turn)) => turn.items.push(item),
        }
    }
    out
}

/// A tool in one line: its name and what it was given, how it ended when it
/// failed, and the files it changed with their lines.
fn tool_line(tool: &Tool, root: &str) -> String {
    let stats = changed::stats_of(&tool.changes, root);
    let what = if stats.is_empty() {
        clip_head(&one_line(&tool.summary), TOOL_LINE)
    } else {
        let shown: Vec<String> = stats
            .iter()
            .take(3)
            .map(|stat| {
                format!(
                    "{} ({})",
                    stat.path,
                    changed::counts(stat.added, stat.removed)
                )
            })
            .collect();
        let more = stats.len().saturating_sub(3);
        let mut files = shown.join(", ");
        if more > 0 {
            files.push_str(&format!(" and {more} more"));
        }
        clip_head(&files, TOOL_LINE)
    };
    let failed = tool.outcome.as_ref().is_some_and(|outcome| !outcome.ok);
    format!(
        "{}: {what}{}",
        tool.name,
        if failed { " (failed)" } else { "" }
    )
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` up to `most` characters, with an ellipsis when it was longer.
fn clip_head(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_owned();
    }
    let kept: String = text.chars().take(most.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// `text` up to about `most` characters: its beginning and its end, with
/// what is left out between them said.
fn clip(text: &str, most: usize) -> String {
    let all: Vec<char> = text.trim().chars().collect();
    if all.len() <= most {
        return all.into_iter().collect();
    }
    let tail = most / 3;
    let head = most - tail;
    format!(
        "{}\n[\u{2026} {} characters left out \u{2026}]\n{}",
        all[..head].iter().collect::<String>().trim_end(),
        all.len() - head - tail,
        all[all.len() - tail..]
            .iter()
            .collect::<String>()
            .trim_start()
    )
}

/// The agent's list of things to do as it stands, when it has one.
fn todos_part(items: &[Item]) -> String {
    let Some(Item::Todos(todos)) = items
        .iter()
        .rev()
        .find(|item| matches!(item, Item::Todos(_)))
    else {
        return String::new();
    };
    if todos.is_empty() {
        return String::new();
    }
    let mut out = String::from("Things to do, as the agent had them:\n");
    for todo in todos.iter().take(MAX_TODOS) {
        let mark = match todo.status {
            TodoStatus::Completed => "x",
            TodoStatus::InProgress => "~",
            TodoStatus::Pending => " ",
        };
        out.push_str(&format!(
            "- [{mark}] {}\n",
            clip_head(&one_line(&todo.text), SUMMARY_LINE)
        ));
    }
    if todos.len() > MAX_TODOS {
        out.push_str(&format!("- and {} more\n", todos.len() - MAX_TODOS));
    }
    out.push('\n');
    out
}

/// The files the conversation changed, when it changed any.
fn files_part(items: &[Item], root: &str) -> String {
    let stats = changed::from_items(items, root);
    if stats.is_empty() {
        return String::new();
    }
    let mut out = String::from("Files changed so far (+lines added, -lines removed):\n");
    for stat in stats.iter().take(MAX_FILES) {
        out.push_str(&format!(
            "- {} {}\n",
            clip_head(&stat.path, SUMMARY_LINE),
            changed::counts(stat.added, stat.removed)
        ));
    }
    if stats.len() > MAX_FILES {
        out.push_str(&format!("- and {} more\n", stats.len() - MAX_FILES));
    }
    out.push('\n');
    out
}

/// The block that tells `to` what was said between the person and the
/// agents before it, from the `items` of the conversation (`from` is the
/// agent that answered last, `root` the folder, for the names of files), in
/// at most `cap` characters when the parts that always go in fit in them.
/// `None` when the person said nothing yet: there is nothing to hand over.
pub fn text(items: &[Item], from: AgentId, to: AgentId, root: &str, cap: usize) -> Option<String> {
    let turns = turns(items);
    if !items.iter().any(|item| matches!(item, Item::User { .. })) {
        return None;
    }
    let head = format!(
        "{}\nThe person was working with {} in this folder and now continues with you ({}). \
         What follows is the conversation so far, from the application's own transcript: use it \
         as background to carry on from where it stopped. Do not repeat it back, and do not \
         redo what is finished. Their new message comes after this block.\n\n",
        marker(from, to),
        crate::format::agent_name(from),
        crate::format::agent_name(to),
    );
    let tail = format!("</{TAG}>\n");
    let fixed = format!("{}{}", todos_part(items), files_part(items, root));
    let used = |text: &str| text.chars().count();
    let mut room = cap.saturating_sub(used(&head) + used(&tail) + used(&fixed) + HEADINGS);

    // The newest turns, whole, as many as fit; a turn that does not fit is
    // tried with shorter messages, and then the ones before it are lines.
    // When some turns are left to lines, a quarter of the room is theirs.
    let pick = |budget: usize| {
        let mut left = budget;
        let mut recent: Vec<String> = Vec::new();
        let mut whole_from = turns.len();
        for (at, turn) in turns.iter().enumerate().rev() {
            let fitted = MESSAGE_CAPS.iter().enumerate().find_map(|(tier, most)| {
                let text = turn.whole(*most, root);
                (used(&text) < left).then_some((tier, text))
            });
            let Some((tier, text)) = fitted else {
                break;
            };
            left -= used(&text) + 1;
            recent.push(text);
            whole_from = at;
            if tier > 0 {
                break;
            }
        }
        recent.reverse();
        (recent, whole_from, budget - left)
    };
    let (mut recent, mut whole_from, mut spent) = pick(room);
    if whole_from > 0 {
        (recent, whole_from, spent) = pick(room * 3 / 4);
    }
    room -= spent;

    // The ones before: a line each, the oldest left out first.
    let note = |left: usize| format!("({left} earlier turns left out)\n",);
    let earlier = &turns[..whole_from];
    let mut lines: Vec<String> = Vec::new();
    let mut taken = earlier.len();
    if !earlier.is_empty() {
        room = room.saturating_sub(used(&note(earlier.len())) + 32);
        for (at, turn) in earlier.iter().enumerate().rev() {
            let line = turn.line();
            if used(&line) > room {
                break;
            }
            room -= used(&line);
            lines.push(line);
            taken = at;
        }
        lines.reverse();
    }

    let mut out = head;
    out.push_str(&fixed);
    if !earlier.is_empty() {
        out.push_str("Earlier in the conversation, in a line each:\n");
        if taken > 0 {
            out.push_str(&note(taken));
        }
        for line in &lines {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !recent.is_empty() {
        out.push_str("The latest turns, as they were:\n\n");
        out.push_str(&recent.join("\n"));
        out.push('\n');
    }
    out.push_str(&tail);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::model::{Outcome, Tone};
    use leon_turn::{FileChange, Todo};

    const CLAUDE: AgentId = AgentId::CLAUDE;
    const CODEX: AgentId = AgentId::CODEX;

    fn user(text: &str) -> Item {
        Item::User {
            text: text.to_owned(),
            at: None,
        }
    }

    fn agent(text: &str) -> Item {
        Item::Agent {
            text: text.to_owned(),
            streaming: false,
            at: None,
        }
    }

    fn tool(name: &str, summary: &str) -> Item {
        Item::Tool(Tool {
            id: summary.to_owned(),
            name: name.to_owned(),
            summary: summary.to_owned(),
            detail: "a very long detail that is never handed over".to_owned(),
            changes: Vec::new(),
            outcome: Some(Outcome {
                ok: true,
                summary: String::new(),
                detail: String::new(),
            }),
        })
    }

    fn edit(path: &str, content: &str) -> Item {
        Item::Tool(Tool {
            id: path.to_owned(),
            name: "Edit".to_owned(),
            summary: path.to_owned(),
            detail: String::new(),
            changes: vec![FileChange::written(path, content)],
            outcome: None,
        })
    }

    fn block(items: &[Item], cap: usize) -> String {
        text(items, CLAUDE, CODEX, "/work", cap).expect("something was said")
    }

    #[test]
    fn nothing_said_is_nothing_to_hand_over() {
        assert_eq!(text(&[], CLAUDE, CODEX, "/work", CAP), None);
        let only_notes = [Item::Notice {
            tone: Tone::Plain,
            text: "stopped".into(),
        }];
        assert_eq!(text(&only_notes, CLAUDE, CODEX, "/work", CAP), None);
    }

    #[test]
    fn the_block_is_wrapped_in_its_tag_and_names_both_agents() {
        let out = block(&[user("hi"), agent("hello")], CAP);
        assert!(out.starts_with(&marker(CLAUDE, CODEX)), "{out}");
        assert!(out.trim_end().ends_with("</leon-handoff>"), "{out}");
        assert!(out.contains("Claude Code"), "{out}");
        assert!(out.contains("Codex"), "{out}");
        assert_eq!(parse_marker(&out), Some((CLAUDE, CODEX)));
        assert_eq!(
            parse_marker(&format!("{out}\n\nnext message")),
            Some((CLAUDE, CODEX))
        );
        assert_eq!(parse_marker("just words"), None);
        assert_eq!(parse_marker("<leon-handoff from=\"?\" to=\"codex\">"), None);
    }

    #[test]
    fn what_was_said_comes_verbatim_and_in_order() {
        let out = block(
            &[
                user("first question"),
                agent("first answer, with `code`"),
                user("second question"),
                agent("second answer"),
            ],
            CAP,
        );
        let at = |needle: &str| {
            out.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {out}"))
        };
        assert!(at("first question") < at("first answer, with `code`"));
        assert!(at("first answer, with `code`") < at("second question"));
        assert!(at("second question") < at("second answer"));
    }

    #[test]
    fn a_tool_is_one_line_and_an_edit_says_its_files_and_lines() {
        let out = block(
            &[
                user("fix it"),
                tool("Bash", "cargo test"),
                edit("/work/src/a.rs", "one\ntwo\n"),
                agent("done"),
            ],
            CAP,
        );
        assert!(out.contains("Bash: cargo test"), "{out}");
        assert!(out.contains("Edit: src/a.rs (+2 -0)"), "{out}");
        assert!(!out.contains("never handed over"), "{out}");
    }

    #[test]
    fn a_tool_that_failed_says_so() {
        let mut failed = tool("Bash", "make");
        if let Item::Tool(tool) = &mut failed {
            tool.outcome = Some(Outcome {
                ok: false,
                summary: "exit 2".into(),
                detail: String::new(),
            });
        }
        let out = block(&[user("build"), failed, agent("it broke")], CAP);
        assert!(out.contains("Bash: make (failed)"), "{out}");
    }

    #[test]
    fn the_todos_as_they_stand_and_the_files_changed_are_in() {
        let out = block(
            &[
                user("do the list"),
                Item::Todos(vec![
                    Todo {
                        text: "write the test".into(),
                        status: TodoStatus::Completed,
                    },
                    Todo {
                        text: "write the code".into(),
                        status: TodoStatus::InProgress,
                    },
                    Todo {
                        text: "ship it".into(),
                        status: TodoStatus::Pending,
                    },
                ]),
                edit("/work/src/a.rs", "x\n"),
                edit("/work/src/b.rs", "y\n"),
                edit("/work/src/a.rs", "z\n"),
                agent("ok"),
            ],
            CAP,
        );
        assert!(out.contains("- [x] write the test"), "{out}");
        assert!(out.contains("- [~] write the code"), "{out}");
        assert!(out.contains("- [ ] ship it"), "{out}");
        // Each file once, with all its lines.
        assert_eq!(out.matches("src/a.rs +2 -0").count(), 1, "{out}");
        assert!(out.contains("src/b.rs +1 -0"), "{out}");
    }

    #[test]
    fn without_todos_or_changes_those_parts_are_left_out() {
        let out = block(&[user("hi"), agent("hello")], CAP);
        assert!(!out.contains("Things to do"), "{out}");
        assert!(!out.contains("Files changed"), "{out}");
    }

    /// Turns of 300 characters each, numbered.
    fn many(count: usize) -> Vec<Item> {
        (0..count)
            .flat_map(|n| {
                [
                    user(&format!("question {n:03} {}", "q".repeat(100))),
                    agent(&format!("answer {n:03} {}", "a".repeat(180))),
                ]
            })
            .collect()
    }

    #[test]
    fn past_the_cap_the_newest_turns_stay_whole_and_the_oldest_are_a_line_each() {
        let items = many(60);
        let out = block(&items, 12_000);
        assert!(out.chars().count() <= 12_000, "{}", out.chars().count());
        // The newest is whole.
        assert!(out.contains(&format!("answer 059 {}", "a".repeat(180))));
        assert!(out.contains(&format!("question 059 {}", "q".repeat(100))));
        // An old one is a line: its words are there, its long tail is not.
        let old = out
            .lines()
            .find(|line| line.contains("question 028"))
            .unwrap_or_else(|| panic!("{out}"));
        assert!(old.contains("answer 028"), "{old}");
        // The oldest of all were left out, and it is said.
        assert!(!out.contains("question 000"), "{out}");
        assert!(out.contains("earlier turns left out"), "{out}");
        // What stayed is in order.
        let positions: Vec<usize> = ["question 025", "question 040", "question 059"]
            .iter()
            .map(|needle| out.find(needle).unwrap_or_else(|| panic!("{needle}")))
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn the_default_cap_holds_a_long_conversation() {
        let out = block(&many(400), CAP);
        assert!(out.chars().count() <= CAP, "{}", out.chars().count());
        assert!(out.contains("question 399"));
    }

    #[test]
    fn one_enormous_message_is_cut_in_the_middle_and_the_newest_turn_is_still_in() {
        let long = format!("start {} end", "x".repeat(100_000));
        let out = block(
            &[user("old"), agent("old answer"), user("go"), agent(&long)],
            12_000,
        );
        assert!(out.chars().count() <= 12_000, "{}", out.chars().count());
        assert!(out.contains("start "), "the head stays");
        assert!(out.contains(" end"), "the tail stays");
        assert!(out.contains("characters left out"), "{out}");
        assert!(out.contains("go"));
    }

    #[test]
    fn a_switch_already_made_is_not_the_persons_message_and_notes_are_skipped() {
        let out = block(
            &[
                user("one"),
                agent("two"),
                Item::Switched {
                    from: CLAUDE,
                    to: CODEX,
                },
                Item::Notice {
                    tone: Tone::Bad,
                    text: "boom".into(),
                },
                user("three"),
            ],
            CAP,
        );
        assert!(out.contains("one") && out.contains("two") && out.contains("three"));
        assert!(!out.contains("boom"), "{out}");
    }
}
