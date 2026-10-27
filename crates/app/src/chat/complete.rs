//! What a conversation's field offers while something is being named.
//!
//! Three things are named rather than written out, the same way whatever the
//! agent is:
//!
//! * a **command**, after a slash that opens the prompt: Leon's own four
//!   (`/model`, `/effort`, `/mode`, `/voice`), then whatever the agent lists
//!   as its own (its commands, its skills, the prompts somebody saved for
//!   it);
//! * a **file** of the folder the agent works in, after an `@` anywhere;
//! * a **mode**, a **model** or an **effort** (how hard the model thinks),
//!   in the list a chip of the field opens;
//! * a **speech model** for dictation, in the list `/voice` opens (and the
//!   field's own button, until there is a model to dictate with).
//!
//! This module is the part without a window: which of them the text asks
//! for where the cursor is ([`token`]), and the rows that answer what was
//! typed, best first. `ui/chat_menu.rs` draws them and does what a row says.

use leon_turn::{AgentCommand, AgentModel, AgentOptions};
use leon_voice::catalog;

use crate::fuzzy;

/// The most rows a list shows. More than this nobody reads: the rest is
/// asked for by typing more.
pub const ROWS: usize = 50;

/// What the text asks for where the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A command: a slash that opens the prompt.
    Slash,
    /// A file: an `@` that opens a word.
    At,
}

/// The word being typed, when it names something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// What it names.
    pub trigger: Trigger,
    /// Where its sign is in the text, in bytes.
    pub start: usize,
    /// What follows the sign up to the cursor.
    pub query: String,
}

/// The word that ends at `cursor` (a place in `text`, in bytes), when it
/// names something. A slash only does so as the first thing of the prompt,
/// so a path in the middle of a sentence is a path.
pub fn token(text: &str, cursor: usize) -> Option<Token> {
    let before = text.get(..cursor)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(at, c)| at + c.len_utf8());
    let word = &before[start..];
    let trigger = match word.chars().next()? {
        '/' if before[..start].trim().is_empty() => Trigger::Slash,
        '@' => Trigger::At,
        _ => return None,
    };
    Some(Token {
        trigger,
        start,
        query: word[1..].to_owned(),
    })
}

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Opens the list of the agent's models.
    Models,
    /// Opens the list of the efforts of the model in use.
    Efforts,
    /// Opens the list of the agent's modes.
    Modes,
    /// Opens the list of the speech models of dictation.
    Voices,
    /// Writes this in place of the word: what asks the agent for a command.
    Command(String),
    /// Writes this file's path in place of the word, after an `@`.
    File(String),
    /// The conversation works in this mode.
    Mode(String),
    /// The conversation answers with this model; `None` is the agent's own.
    Model(Option<String>),
    /// The model thinks this hard; `None` is as the agent is set up.
    Effort(Option<String>),
    /// Dictation uses this speech model, downloaded first when it is not
    /// on this computer.
    Voice(String),
}

/// One thing a list offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What it is called, as it would be typed.
    pub label: String,
    /// What it is, in a sentence.
    pub detail: Option<String>,
    /// What may follow it, or whose it is.
    pub note: Option<String>,
    /// It is the one in use.
    pub current: bool,
    /// Choosing it lets the agent do anything without asking.
    pub warn: bool,
    /// What choosing it does.
    pub pick: Pick,
}

/// The commands that answer `query`: Leon's own first (a list of the
/// agent's models, of the `efforts` of the one in use, or of its modes, when
/// there is any to choose from), then the agent's. One of the agent's that
/// is called like one of Leon's is left out: in its terminal it opens a
/// picker, which is what Leon's does.
pub fn command_rows(
    query: &str,
    options: Option<&AgentOptions>,
    efforts: &[String],
    agent: &[AgentCommand],
) -> Vec<Row> {
    let mut rows = Vec::new();
    if options.is_some_and(|options| !options.models.is_empty()) {
        rows.push(Row {
            label: "/model".to_owned(),
            detail: Some("Choose the model that answers.".to_owned()),
            note: Some("BUILT IN".to_owned()),
            current: false,
            warn: false,
            pick: Pick::Models,
        });
    }
    if !efforts.is_empty() {
        rows.push(Row {
            label: "/effort".to_owned(),
            detail: Some("Choose how hard the model thinks.".to_owned()),
            note: Some("BUILT IN".to_owned()),
            current: false,
            warn: false,
            pick: Pick::Efforts,
        });
    }
    if options.is_some_and(|options| options.modes.len() > 1) {
        rows.push(Row {
            label: "/mode".to_owned(),
            detail: Some("Choose what the agent may do without asking.".to_owned()),
            note: Some("BUILT IN".to_owned()),
            current: false,
            warn: false,
            pick: Pick::Modes,
        });
    }
    rows.push(Row {
        label: "/voice".to_owned(),
        detail: Some("Choose the speech model you dictate with.".to_owned()),
        note: Some("BUILT IN".to_owned()),
        current: false,
        warn: false,
        pick: Pick::Voices,
    });
    let own: Vec<String> = rows.iter().map(|row| row.label.clone()).collect();
    rows.extend(
        agent
            .iter()
            .filter(|command| !own.contains(&format!("/{}", command.name)))
            .map(|command| Row {
                label: command.insert.clone(),
                detail: command.description.clone(),
                note: command.hint.clone(),
                current: false,
                warn: false,
                pick: Pick::Command(command.insert.clone()),
            }),
    );
    // Ranked by the name alone, without its sign: `/re` finds `$review` too.
    let mut rows = fuzzy::rank(query, rows, |row| row.label[1..].to_owned());
    rows.truncate(ROWS);
    rows
}

/// The files that answer `query`, best first; with nothing typed, the first
/// of the list.
pub fn file_rows(query: &str, files: &[String]) -> Vec<Row> {
    let row = |path: &String| Row {
        label: path.clone(),
        detail: None,
        note: None,
        current: false,
        warn: false,
        pick: Pick::File(path.clone()),
    };
    if query.is_empty() {
        return files.iter().take(ROWS).map(row).collect();
    }
    let mut scored: Vec<(u32, &String)> = files
        .iter()
        .filter_map(|path| Some((file_score(query, path)?, path)))
        .collect();
    scored.sort_by_key(|(found, _)| std::cmp::Reverse(*found));
    scored
        .into_iter()
        .take(ROWS)
        .map(|(_, path)| row(path))
        .collect()
}

/// How well `query` finds `path`: the file's own name counts for more than
/// the folders in front of it, and a query with a slash is matched on the
/// whole path.
fn file_score(query: &str, path: &str) -> Option<u32> {
    if query.contains('/') {
        return fuzzy::score(query, path);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let by_name = fuzzy::score(query, name).map(|found| found + 1000);
    by_name.into_iter().chain(fuzzy::score(query, path)).max()
}

/// The agent's modes that answer `query`, in its own order when nothing is
/// typed; `current` is the one the conversation works in.
pub fn mode_rows(query: &str, options: &AgentOptions, current: Option<&str>) -> Vec<Row> {
    let current = current.or(options.default_mode.as_deref());
    let rows = options
        .modes
        .iter()
        .map(|mode| Row {
            label: mode.label.clone(),
            detail: mode.description.clone(),
            note: None,
            current: current == Some(mode.id.as_str()),
            warn: mode.dangerous,
            pick: Pick::Mode(mode.id.clone()),
        })
        .collect();
    let mut rows = fuzzy::rank(query, rows, |row: &Row| row.label.clone());
    rows.truncate(ROWS);
    rows
}

/// The agent's models that answer `query`, after the agent's own choice;
/// `current` is the one the conversation asked for, if it asked.
pub fn model_rows(query: &str, options: &AgentOptions, current: Option<&str>) -> Vec<Row> {
    // What "default" is, where the agent said.
    let own = model_shown(Some(options), None, None);
    let mut rows = vec![Row {
        label: "Default".to_owned(),
        detail: Some(match own {
            Some(own) => format!("{own}: the one the agent is set up with."),
            None => "The one the agent is set up with.".to_owned(),
        }),
        note: None,
        current: current.is_none(),
        warn: false,
        pick: Pick::Model(None),
    }];
    rows.extend(options.models.iter().map(|model| Row {
        label: model.label.clone(),
        detail: model.description.clone(),
        note: (model.label != model.id).then(|| model.id.clone()),
        current: current == Some(model.id.as_str()),
        warn: false,
        pick: Pick::Model(Some(model.id.clone())),
    }));
    // A model is found by its name or by its id.
    let mut rows = fuzzy::rank(query, rows, |row: &Row| match &row.note {
        Some(id) => format!("{} {id}", row.label),
        None => row.label.clone(),
    });
    rows.truncate(ROWS);
    rows
}

/// The model of the agent's list that `name` is: the one with that id, or
/// one that stands for it (`opus` for `claude-opus-5-5`, which is what a
/// turn says it answers with). Among several that stand for it, the agent's
/// own choice, then one that is not "whatever is default".
pub fn model_named<'a>(options: &'a AgentOptions, name: &str) -> Option<&'a AgentModel> {
    let by_id = options.models.iter().find(|model| model.id == name);
    let stands_for = |model: &&AgentModel| model.resolves_to.as_deref() == Some(name);
    by_id
        .or_else(|| {
            options
                .models
                .iter()
                .filter(stands_for)
                .find(|model| Some(&model.id) == options.default_model.as_ref())
        })
        .or_else(|| {
            options
                .models
                .iter()
                .filter(stands_for)
                .find(|model| model.id != "default")
        })
        .or_else(|| options.models.iter().find(stands_for))
}

/// The model a turn would answer with: the one `asked` for, else the one the
/// agent says is `answering`, else the one it is set up with, else the first
/// it lists, which is its own choice wherever it marks one.
pub fn model_in_use<'a>(
    options: &'a AgentOptions,
    asked: Option<&str>,
    answering: Option<&str>,
) -> Option<&'a AgentModel> {
    match asked {
        // One asked for that is not listed has no efforts anybody knows of.
        Some(asked) => options.models.iter().find(|model| model.id == asked),
        None => answering
            .and_then(|name| model_named(options, name))
            .or_else(|| {
                options
                    .default_model
                    .as_deref()
                    .and_then(|name| model_named(options, name))
            })
            .or(options.models.first()),
    }
}

/// The name of the model the next turn answers with, for whoever looks at
/// the conversation: the one asked for, else the one the last turn said it
/// answered with, else the one the agent is set up with. A model nobody
/// lists is named as it is written. `None` until anything is known.
pub fn model_shown(
    options: Option<&AgentOptions>,
    asked: Option<&str>,
    answering: Option<&str>,
) -> Option<String> {
    let label = |name: &str| {
        options
            .and_then(|options| model_named(options, name))
            .map_or_else(|| name.to_owned(), |model| model.label.clone())
    };
    asked
        .or(answering)
        .or(options.and_then(|options| options.default_model.as_deref()))
        .map(label)
}

/// Who an answer is from, over it: the model the agent said it answered
/// with (`answering`), exactly as it said it (`claude-opus-5-5`), in
/// capitals; else the agent's own tag, as before any turn said its model.
pub fn answered_by(answering: Option<&str>, agent: &str) -> String {
    answering
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map_or_else(|| agent.to_owned(), str::to_uppercase)
}

/// The effort the next turn thinks with: the one asked for, else the one the
/// agent is set up with. `None` when neither is known.
pub fn effort_shown<'a>(
    options: Option<&'a AgentOptions>,
    asked: Option<&'a str>,
) -> Option<&'a str> {
    asked.or(options.and_then(|options| options.default_effort.as_deref()))
}

/// The efforts a turn can be asked for, the lightest first: those of
/// [`model_in_use`], none when it has no such setting.
pub fn efforts<'a>(
    options: Option<&'a AgentOptions>,
    asked: Option<&str>,
    answering: Option<&str>,
) -> &'a [String] {
    options
        .and_then(|options| model_in_use(options, asked, answering))
        .map_or(&[], |model| model.efforts.as_slice())
}

/// Words for an effort: the agents name them in one lowercase word.
pub fn effort_label(effort: &str) -> String {
    match effort {
        "xhigh" => "Extra high".to_owned(),
        _ => {
            let mut letters = effort.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        }
    }
}

/// What an effort the agents share means, in a sentence.
fn effort_detail(effort: &str) -> Option<&'static str> {
    Some(match effort {
        "none" => "No thinking before it answers.",
        "minimal" => "Hardly any thinking: the fastest answers.",
        "low" => "A little thinking: fast, for simple work.",
        "medium" => "Thinks where the work asks for it.",
        "high" => "Thinks things through: slower, for hard work.",
        "xhigh" => "Thinks at length: for the hardest work.",
        "max" => "All the thinking it can do: the slowest.",
        _ => return None,
    })
}

/// The efforts that answer `query`, the lightest first, after the agent's
/// own choice; `current` is the one the conversation asked for, if it asked.
pub fn effort_rows(
    query: &str,
    efforts: &[String],
    current: Option<&str>,
    own: Option<&str>,
) -> Vec<Row> {
    let mut rows = vec![Row {
        label: "Default".to_owned(),
        detail: Some(match own {
            Some(own) => format!("{}: as the agent is set up.", effort_label(own)),
            None => "As the agent is set up.".to_owned(),
        }),
        note: None,
        current: current.is_none(),
        warn: false,
        pick: Pick::Effort(None),
    }];
    rows.extend(efforts.iter().map(|effort| Row {
        label: effort_label(effort),
        detail: effort_detail(effort).map(str::to_owned),
        note: None,
        current: current == Some(effort.as_str()),
        warn: false,
        pick: Pick::Effort(Some(effort.clone())),
    }));
    // By its words, or by the one word the agent calls it.
    let mut rows = fuzzy::rank(query, rows, |row: &Row| match &row.pick {
        Pick::Effort(Some(id)) => format!("{} {id}", row.label),
        _ => row.label.clone(),
    });
    rows.truncate(ROWS);
    rows
}

/// What is known of the speech models when their list is drawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct Voices<'a> {
    /// The language the person dictates in; `None` is whichever is heard.
    pub language: Option<&'a str>,
    /// The ids of the models on this computer.
    pub installed: &'a [String],
    /// The id of the model dictation uses.
    pub in_use: Option<&'a str>,
    /// The model being downloaded, and the bytes there are of it.
    pub downloading: Option<(&'a str, u64)>,
}

/// The speech models offered for the language, the lightest first, that
/// answer `query`: each with its size until it is on this computer, and how
/// far its download is while there is one.
pub fn voice_rows(query: &str, voices: Voices) -> Vec<Row> {
    let rows = catalog::offered(voices.language)
        .into_iter()
        .map(|model| {
            let installed = voices.installed.iter().any(|id| id == model.id);
            let note = match voices.downloading {
                Some((id, bytes)) if id == model.id => {
                    format!(
                        "DOWNLOADING {}%",
                        bytes.min(model.bytes) * 100 / model.bytes
                    )
                }
                _ if installed => "ON THIS COMPUTER".to_owned(),
                _ => format!("DOWNLOAD \u{b7} {}", catalog::megabytes(model.bytes)),
            };
            Row {
                label: model.label.to_owned(),
                detail: Some(model.detail.to_owned()),
                note: Some(note),
                current: voices.in_use == Some(model.id),
                warn: false,
                pick: Pick::Voice(model.id.to_owned()),
            }
        })
        .collect();
    fuzzy::rank(query, rows, |row: &Row| row.label.clone())
}

/// `text` with what was `said` written where the cursor is, and where the
/// cursor is afterwards: a space is put between it and a word it would
/// otherwise touch.
pub fn dictated(text: &str, cursor: usize, said: &str) -> (String, usize) {
    let said = said.trim();
    let cursor = cursor.min(text.len());
    if said.is_empty() || !text.is_char_boundary(cursor) {
        return (text.to_owned(), cursor);
    }
    let (before, after) = text.split_at(cursor);
    let open = |c: char| c.is_whitespace() || matches!(c, '(' | '[' | '"' | '\'');
    let lead = if before.chars().next_back().is_none_or(open) {
        ""
    } else {
        " "
    };
    let trail = if after.chars().next().is_none_or(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let end = before.len() + lead.len() + said.len() + trail.len();
    (format!("{before}{lead}{said}{trail}{after}"), end)
}

/// `text` with the word that starts at `start` and ends at `cursor`
/// replaced by `with` and a space, and where the cursor is afterwards.
pub fn replaced(text: &str, start: usize, cursor: usize, with: &str) -> (String, usize) {
    let (start, cursor) = (start.min(text.len()), cursor.min(text.len()));
    let after = &text[cursor..];
    // A space is already there when the word was written in the middle.
    let gap = if after.starts_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let end = start + with.len() + gap.len();
    (format!("{}{with}{gap}{after}", &text[..start]), end)
}

#[cfg(test)]
mod tests {
    use leon_turn::AgentMode;

    use super::*;

    fn slash(query: &str) -> Option<Token> {
        Some(Token {
            trigger: Trigger::Slash,
            start: 0,
            query: query.to_owned(),
        })
    }

    #[test]
    fn a_slash_names_a_command_only_as_the_first_thing_of_the_prompt() {
        assert_eq!(token("/", 1), slash(""));
        assert_eq!(token("/comp", 5), slash("comp"));
        // The cursor may be inside the word: what is before it counts.
        assert_eq!(token("/compact", 3), slash("co"));
        assert_eq!(
            token("  /mo", 5),
            Some(Token {
                trigger: Trigger::Slash,
                start: 2,
                query: "mo".into()
            })
        );
        // Once the command is written, what follows is its own.
        assert_eq!(token("/review the loop", 16), None);
        assert_eq!(token("/review ", 8), None);
        // A path in a sentence is a path.
        assert_eq!(token("open /etc/hosts", 15), None);
        assert_eq!(token("", 0), None);
        assert_eq!(token("hello", 5), None);
    }

    #[test]
    fn an_at_names_a_file_wherever_it_opens_a_word() {
        let at = |start, query: &str| {
            Some(Token {
                trigger: Trigger::At,
                start,
                query: query.to_owned(),
            })
        };
        assert_eq!(token("@", 1), at(0, ""));
        assert_eq!(token("fix @src/ma", 11), at(4, "src/ma"));
        assert_eq!(token("fix\n@lib", 8), at(4, "lib"));
        assert_eq!(token("año @ñu", "año @ñu".len()), at(5, "ñu"));
        // An address is not a file, and a finished name is left alone.
        assert_eq!(token("me@example.com", 14), None);
        assert_eq!(token("fix @src/main.rs now", 20), None);
        // A cursor that is not on a character names nothing.
        assert_eq!(token("ñ", 1), None);
        assert_eq!(token("abc", 9), None);
    }

    fn options() -> AgentOptions {
        AgentOptions {
            modes: vec![
                AgentMode {
                    id: "default".into(),
                    label: "Ask first".into(),
                    description: None,
                    dangerous: false,
                },
                AgentMode {
                    id: "bypassPermissions".into(),
                    label: "Bypass permissions".into(),
                    description: Some("Anything, without asking.".into()),
                    dangerous: true,
                },
            ],
            default_mode: Some("default".into()),
            models: vec![
                AgentModel {
                    id: "opus".into(),
                    label: "Opus".into(),
                    description: None,
                    efforts: vec!["low".into(), "high".into(), "xhigh".into()],
                    resolves_to: Some("claude-opus-5-5".into()),
                },
                AgentModel {
                    id: "anthropic/claude-haiku".into(),
                    label: "claude-haiku · anthropic".into(),
                    description: None,
                    efforts: Vec::new(),
                    resolves_to: None,
                },
            ],
            ..AgentOptions::default()
        }
    }

    /// The same agent, set up to answer with its first model at high effort.
    fn set_up() -> AgentOptions {
        AgentOptions {
            default_model: Some("opus".into()),
            default_effort: Some("high".into()),
            ..options()
        }
    }

    #[test]
    fn what_is_shown_is_what_the_agent_runs_with_until_something_else_is_asked_for() {
        let set_up = set_up();
        // Nothing asked for, no turn yet: the agent's own.
        assert_eq!(
            model_shown(Some(&set_up), None, None).as_deref(),
            Some("Opus")
        );
        assert_eq!(effort_shown(Some(&set_up), None), Some("high"));
        // What a turn says it answers with is the model's own name: it is
        // shown as the entry a person knows it by.
        assert_eq!(
            model_shown(Some(&set_up), None, Some("claude-opus-5-5")).as_deref(),
            Some("Opus")
        );
        // What is asked for here stands before both.
        assert_eq!(
            model_shown(
                Some(&set_up),
                Some("anthropic/claude-haiku"),
                Some("claude-opus-5-5")
            )
            .as_deref(),
            Some("claude-haiku \u{b7} anthropic")
        );
        assert_eq!(effort_shown(Some(&set_up), Some("low")), Some("low"));
        // A model nobody lists is named as it is written.
        assert_eq!(
            model_shown(Some(&set_up), None, Some("my-model")).as_deref(),
            Some("my-model")
        );
        // An agent that said nothing of how it is set up: nothing to show.
        assert_eq!(model_shown(Some(&options()), None, None), None);
        assert_eq!(effort_shown(Some(&options()), None), None);
        assert_eq!(model_shown(None, None, None), None);
    }

    #[test]
    fn an_answer_is_said_to_be_from_the_exact_model_else_from_the_agent() {
        assert_eq!(
            answered_by(Some("claude-opus-5-5"), "CLAUDE CODE"),
            "CLAUDE-OPUS-5-5"
        );
        assert_eq!(answered_by(Some(" "), "CODEX"), "CODEX");
        assert_eq!(answered_by(None, "CODEX"), "CODEX");
    }

    #[test]
    fn the_efforts_offered_are_those_of_the_model_the_agent_runs_with() {
        // Set up with a model that has none, nothing asked for.
        let plain = AgentOptions {
            default_model: Some("anthropic/claude-haiku".into()),
            ..options()
        };
        assert!(efforts(Some(&plain), None, None).is_empty());
        assert_eq!(efforts(Some(&set_up()), None, None).len(), 3);
        // The model a turn answered with, by its own name.
        assert_eq!(
            model_in_use(&plain, None, Some("claude-opus-5-5")).map(|model| model.id.as_str()),
            Some("opus")
        );
    }

    #[test]
    fn the_lists_say_what_default_is() {
        let set_up = set_up();
        assert_eq!(
            model_rows("", &set_up, None)[0].detail.as_deref(),
            Some("Opus: the one the agent is set up with.")
        );
        assert_eq!(
            model_rows("", &options(), None)[0].detail.as_deref(),
            Some("The one the agent is set up with.")
        );
        let listed = efforts(Some(&set_up), None, None);
        assert_eq!(
            effort_rows("", listed, None, Some("high"))[0]
                .detail
                .as_deref(),
            Some("High: as the agent is set up.")
        );
    }

    fn labels(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.label.as_str()).collect()
    }

    #[test]
    fn leons_own_commands_come_first_and_shadow_the_agents_of_the_same_name() {
        let agent = [
            AgentCommand::slash("model", Some("Its own picker"), None),
            AgentCommand::slash("compact", Some("Keep a summary"), Some("[instructions]")),
            AgentCommand {
                insert: "$humanizer".into(),
                ..AgentCommand::slash("humanizer", None, None)
            },
        ];
        let all = command_rows("", Some(&options()), &[], &agent);
        assert_eq!(
            labels(&all),
            ["/model", "/mode", "/voice", "/compact", "$humanizer"]
        );
        assert_eq!(all[0].pick, Pick::Models);
        assert_eq!(all[1].pick, Pick::Modes);
        assert_eq!(all[2].pick, Pick::Voices);
        assert_eq!(all[3].pick, Pick::Command("/compact".into()));
        assert_eq!(all[3].note.as_deref(), Some("[instructions]"));
        // A model that can be told how hard to think adds a third, which
        // shadows the agent's own as the others do.
        let thinks = [AgentCommand::slash("effort", None, None)];
        let with = command_rows("", Some(&options()), &["low".to_owned()], &thinks);
        assert_eq!(labels(&with), ["/model", "/effort", "/mode", "/voice"]);
        assert_eq!(with[1].pick, Pick::Efforts);
        let without = command_rows("", Some(&options()), &[], &thinks);
        assert_eq!(without[3].pick, Pick::Command("/effort".into()));

        assert_eq!(
            labels(&command_rows("mo", Some(&options()), &[], &agent)),
            ["/mode", "/model"]
        );
        // A skill is found by its name, whatever sign asks for it.
        assert_eq!(
            labels(&command_rows("hum", Some(&options()), &[], &agent)),
            ["$humanizer"]
        );
        assert!(command_rows("zzz", Some(&options()), &[], &agent).is_empty());

        // An agent that was not asked yet, or has one mode and no list of
        // models, has nothing of Leon's to choose: its own command stays.
        let bare = command_rows("", None, &[], &agent);
        assert_eq!(
            labels(&bare),
            ["/voice", "/model", "/compact", "$humanizer"]
        );
        assert_eq!(bare[1].pick, Pick::Command("/model".into()));
    }

    #[test]
    fn files_are_found_by_their_name_before_their_folders() {
        let files: Vec<String> = [
            "docs/main.md",
            "src/main.rs",
            "src/ui/chat.rs",
            "main/notes.txt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            labels(&file_rows("", &files)),
            files.iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert_eq!(labels(&file_rows("chat", &files)), ["src/ui/chat.rs"]);
        let found = file_rows("main.rs", &files);
        assert_eq!(found[0].pick, Pick::File("src/main.rs".into()));
        assert_eq!(
            labels(&file_rows("src/", &files)),
            ["src/main.rs", "src/ui/chat.rs"]
        );
        let many: Vec<String> = (0..200).map(|n| format!("f{n}.rs")).collect();
        assert_eq!(file_rows("", &many).len(), ROWS);
        assert_eq!(file_rows("f", &many).len(), ROWS);
    }

    #[test]
    fn modes_and_models_say_which_is_in_use() {
        let modes = mode_rows("", &options(), None);
        assert_eq!(labels(&modes), ["Ask first", "Bypass permissions"]);
        assert!(modes[0].current && !modes[1].current);
        assert!(modes[1].warn);
        let asked = mode_rows("", &options(), Some("bypassPermissions"));
        assert!(asked[1].current && !asked[0].current);
        assert_eq!(
            labels(&mode_rows("byp", &options(), None)),
            ["Bypass permissions"]
        );

        let models = model_rows("", &options(), None);
        assert_eq!(
            labels(&models),
            ["Default", "Opus", "claude-haiku · anthropic"]
        );
        assert!(models[0].current);
        assert_eq!(models[0].pick, Pick::Model(None));
        let asked = model_rows("", &options(), Some("opus"));
        assert!(asked[1].current && !asked[0].current);
        // By its name, or by the id a person knows it by.
        assert_eq!(
            labels(&model_rows("haiku", &options(), None)),
            ["claude-haiku · anthropic"]
        );
        assert_eq!(
            model_rows("anthropic/", &options(), None)[0].pick,
            Pick::Model(Some("anthropic/claude-haiku".into()))
        );
    }

    #[test]
    fn the_efforts_are_those_of_the_model_a_turn_would_answer_with() {
        let options = options();
        let of = |asked, answering| efforts(Some(&options), asked, answering).to_vec();
        // Nothing asked and nothing said: the first it lists is its own.
        assert_eq!(of(None, None), ["low", "high", "xhigh"]);
        assert_eq!(of(Some("opus"), None), ["low", "high", "xhigh"]);
        // The one asked for counts before the one that answered last.
        assert!(of(Some("anthropic/claude-haiku"), Some("opus")).is_empty());
        assert!(of(None, Some("anthropic/claude-haiku")).is_empty());
        // What it says is answering may be a name it does not list.
        assert_eq!(of(None, Some("claude-opus-5-5")), ["low", "high", "xhigh"]);
        assert!(of(Some("gone"), None).is_empty());
        assert!(efforts(None, None, None).is_empty());

        let rows = effort_rows("", &of(None, None), Some("high"), None);
        assert_eq!(labels(&rows), ["Default", "Low", "High", "Extra high"]);
        assert!(rows[2].current && !rows[0].current);
        assert_eq!(rows[0].pick, Pick::Effort(None));
        assert_eq!(rows[3].pick, Pick::Effort(Some("xhigh".into())));
        assert!(effort_rows("", &of(None, None), None, None)[0].current);
        // By its words, or by the word the agent has for it.
        assert_eq!(
            labels(&effort_rows("extra", &of(None, None), None, None)),
            ["Extra high"]
        );
        assert_eq!(
            labels(&effort_rows("xh", &of(None, None), None, None)),
            ["Extra high"]
        );
        assert_eq!(effort_label("minimal"), "Minimal");
        assert_eq!(effort_label(""), "");
    }

    #[test]
    fn the_speech_models_are_those_of_the_language_with_what_is_known_of_each() {
        let installed = ["base".to_owned()];
        let rows = voice_rows(
            "",
            Voices {
                language: Some("es"),
                installed: &installed,
                in_use: Some("base"),
                downloading: Some(("small", 95_042_743)),
            },
        );
        assert_eq!(labels(&rows), ["Fast", "Balanced", "Accurate"]);
        let notes: Vec<_> = rows.iter().map(|row| row.note.clone().unwrap()).collect();
        assert_eq!(
            notes,
            [
                "DOWNLOAD \u{b7} 78 MB",
                "ON THIS COMPUTER",
                "DOWNLOADING 49%"
            ]
        );
        assert!(rows[1].current && !rows[0].current);
        assert_eq!(rows[2].pick, Pick::Voice("small".into()));
        // English has models of its own, and so its own downloads.
        let english = voice_rows(
            "acc",
            Voices {
                language: Some("en"),
                installed: &installed,
                ..Voices::default()
            },
        );
        assert_eq!(english.len(), 1);
        assert_eq!(english[0].pick, Pick::Voice("small.en".into()));
        assert_eq!(english[0].note.as_deref(), Some("DOWNLOAD \u{b7} 488 MB"));
    }

    #[test]
    fn what_was_said_is_written_where_the_cursor_is_without_touching_a_word() {
        assert_eq!(
            dictated("", 0, " fix the bug. "),
            ("fix the bug.".into(), 12)
        );
        assert_eq!(dictated("fix", 3, "the bug"), ("fix the bug".into(), 11));
        assert_eq!(dictated("fix ", 4, "the bug"), ("fix the bug".into(), 11));
        assert_eq!(
            dictated("fix now", 3, "the bug"),
            ("fix the bug now".into(), 11)
        );
        assert_eq!(dictated("fixnow", 3, "it"), ("fix it now".into(), 7));
        assert_eq!(dictated("say (", 5, "this"), ("say (this".into(), 9));
        // Nothing said, or a cursor that is not on a letter: left as it is.
        assert_eq!(dictated("fix", 3, "  "), ("fix".into(), 3));
        assert_eq!(dictated("a\u{f1}o", 2, "x"), ("a\u{f1}o".into(), 2));
        assert_eq!(dictated("ab", 9, "c"), ("ab c".into(), 4));
    }

    #[test]
    fn a_chosen_name_takes_the_place_of_the_word_and_the_cursor_follows_it() {
        assert_eq!(replaced("/co", 0, 3, "/compact"), ("/compact ".into(), 9));
        assert_eq!(
            replaced("fix @ma now", 4, 7, "@src/main.rs"),
            ("fix @src/main.rs now".into(), 16)
        );
        assert_eq!(replaced("@", 0, 1, "@a.rs"), ("@a.rs ".into(), 6));
        // Out of the text: nothing is cut that is not there.
        assert_eq!(replaced("ab", 9, 9, "@x"), ("ab@x ".into(), 5));
    }
}
