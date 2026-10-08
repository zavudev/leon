//! Asking whether a project's agents should share the memory: the pure part.
//!
//! The memory does nothing for a project until its agents are told of it, and
//! that is a step somebody has to choose (`docs/MEMORY.md`). So that the
//! choice is not hidden in the palette, Leon asks once, right after somebody
//! adds a project on this computer by hand. This file holds what needs no
//! window and no disk:
//!
//! * [`decide`]: whether to ask at all, from the facts. Never for a project
//!   Leon found by itself (a burst of discoveries must not raise a queue of
//!   questions), never for another machine's (the memory is this computer's),
//!   never twice in one run, never after an answer, never with the setting
//!   off; and when the project's files already hold the block there is
//!   nothing to ask, only to say.
//! * [`Offer`]: what the question shows, as data the engine worked out off the
//!   interface thread (which files would be written is read from the disk),
//!   and the words made of it ([`Offer::title`], [`BENEFITS`],
//!   [`Offer::change`]).
//! * [`Choice`] and [`choice_of`]: what a key answers.
//!
//! The engine gathers the facts and keeps the offer (`Engine::memory_offer`);
//! the window shows it (`ui/memory_offer.rs`) and hands the answer back to the
//! engine, which is the one that writes.

use leon_core::{MemoryChoice, ProjectId};
use leon_memory::files::Step;

/// The key of the setting that turns the question off.
pub const SETTING_KEY: &str = "memory_offer";

/// What is known when a project has just entered Leon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// Whether the project is on this computer.
    pub local: bool,
    /// Whether somebody added it by hand (opened, cloned or created it), as
    /// opposed to Leon finding it in the history or on a paired computer.
    pub explicit: bool,
    /// Whether the setting lets Leon ask.
    pub setting_on: bool,
    /// What was decided about this root before, if anything.
    pub answered: Option<MemoryChoice>,
    /// Whether it was asked about already since Leon started.
    pub asked_this_run: bool,
    /// Whether one of its instruction files holds the block already.
    pub block_present: bool,
}

/// What to do about a project that has just entered Leon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Ask.
    Ask,
    /// Nothing to ask: its files hold the block (somebody committed it, or
    /// turned it on from a shell). Say so, and remember it is on.
    AlreadyOn,
    /// Say and do nothing.
    Nothing,
}

/// Whether to ask about a project that has just entered Leon.
pub fn decide(facts: &Facts) -> Decision {
    if !facts.local || !facts.explicit {
        return Decision::Nothing;
    }
    if facts.block_present {
        return Decision::AlreadyOn;
    }
    if !facts.setting_on || facts.answered.is_some() || facts.asked_this_run {
        return Decision::Nothing;
    }
    Decision::Ask
}

/// What the question is about, worked out by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The project.
    pub project: ProjectId,
    /// Its name.
    pub name: String,
    /// Its root.
    pub root: String,
    /// The instruction files that are there and would get the block.
    pub writes: Vec<&'static str>,
    /// The instruction file that would be created, when none is there.
    pub creates: Option<&'static str>,
}

/// What turning the memory on is good for, in three lines.
pub const BENEFITS: [&str; 3] = [
    "What one agent learns, the next one knows: Claude Code, Codex, opencode and the rest share it.",
    "It stays on this computer, in Leon's data folder.",
    "Agents save decisions, conventions and discoveries, and find them again by search.",
];

/// The names as a phrase: `A`, `A and B`, `A, B and C`.
fn listed(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [most @ .., last] => format!("{} and {last}", most.join(", ")),
    }
}

impl Offer {
    /// An offer for the project from what turning it on would do to its
    /// instruction files (`leon_memory::files::enable`).
    pub fn new(project: ProjectId, name: String, root: String, steps: &[Step]) -> Self {
        Self {
            project,
            name,
            root,
            writes: steps
                .iter()
                .filter_map(|step| match step {
                    Step::Write { name, .. } => Some(*name),
                    _ => None,
                })
                .collect(),
            creates: steps.iter().find_map(|step| match step {
                Step::Create { name, .. } => Some(*name),
                _ => None,
            }),
        }
    }

    /// Whether turning it on would write anything: when it would not (a link
    /// Leon does not follow is all there is), there is nothing to offer.
    pub fn writes_something(&self) -> bool {
        !self.writes.is_empty() || self.creates.is_some()
    }

    /// The question.
    pub fn title(&self) -> String {
        format!("Turn on agent memory for {}?", self.name)
    }

    /// Exactly what turning it on changes, and how it is undone.
    pub fn change(&self) -> String {
        let what = match (self.writes.as_slice(), self.creates) {
            ([], Some(created)) => format!(
                "Leon creates {created} in {} with a short section that tells agents about the memory",
                self.root
            ),
            (writes, _) => format!(
                "Leon adds a short section to {} in {}",
                listed(writes),
                self.root
            ),
        };
        format!("{what}. Turn off agent memory for this project removes it again, byte for byte.")
    }
}

/// What the person answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Turn it on.
    TurnOn,
    /// Not now: ask again the next time this project is added.
    NotNow,
    /// Never for this project.
    Never,
    /// Do not ask about any project again.
    StopAsking,
}

impl Choice {
    /// Every choice, in the order of the buttons.
    pub const ALL: [Choice; 4] = [
        Choice::TurnOn,
        Choice::NotNow,
        Choice::Never,
        Choice::StopAsking,
    ];

    /// The words on its button.
    pub fn label(self) -> &'static str {
        match self {
            Choice::TurnOn => "TURN IT ON",
            Choice::NotNow => "NOT NOW",
            Choice::Never => "NEVER FOR THIS PROJECT",
            Choice::StopAsking => "DO NOT ASK AGAIN",
        }
    }

    /// The key that chooses it, as it is shown beside the button.
    pub fn key_label(self) -> &'static str {
        match self {
            Choice::TurnOn => "Enter",
            Choice::NotNow => "Esc",
            Choice::Never => "N",
            Choice::StopAsking => "D",
        }
    }
}

/// The choice a bare key makes while the question is up. Any other key is
/// not the question's.
pub fn choice_of(key: &str) -> Option<Choice> {
    match key {
        "enter" => Some(Choice::TurnOn),
        "escape" => Some(Choice::NotNow),
        "n" => Some(Choice::Never),
        "d" => Some(Choice::StopAsking),
        _ => None,
    }
}

/// The status line when a project that was just added has the block already.
pub fn already_on(name: &str) -> String {
    format!("Agent memory is already on for {name}.")
}

/// The status line after "never for this project".
pub fn never_line(name: &str) -> String {
    format!(
        "Leon will not ask about agent memory for {name} again. The palette's Turn on agent memory still works."
    )
}

/// The status line after "do not ask again".
pub const STOPPED_LINE: &str =
    "Leon will not offer agent memory again. Settings, Agents turns the question back on.";

#[cfg(test)]
mod tests {
    use super::*;
    use leon_memory::files::{self, Found};

    fn facts() -> Facts {
        Facts {
            local: true,
            explicit: true,
            setting_on: true,
            answered: None,
            asked_this_run: false,
            block_present: false,
        }
    }

    #[test]
    fn a_project_added_by_hand_on_this_computer_is_asked_about_once() {
        assert_eq!(decide(&facts()), Decision::Ask);
        for (changed, expected) in [
            (
                Facts {
                    local: false,
                    ..facts()
                },
                Decision::Nothing,
            ),
            (
                Facts {
                    explicit: false,
                    ..facts()
                },
                Decision::Nothing,
            ),
            (
                Facts {
                    setting_on: false,
                    ..facts()
                },
                Decision::Nothing,
            ),
            (
                Facts {
                    asked_this_run: true,
                    ..facts()
                },
                Decision::Nothing,
            ),
            (
                Facts {
                    block_present: true,
                    ..facts()
                },
                Decision::AlreadyOn,
            ),
        ] {
            assert_eq!(decide(&changed), expected, "{changed:?}");
        }
        for answered in [MemoryChoice::On, MemoryChoice::Off, MemoryChoice::Never] {
            let facts = Facts {
                answered: Some(answered),
                ..facts()
            };
            assert_eq!(decide(&facts), Decision::Nothing, "{answered:?}");
        }
    }

    #[test]
    fn every_combination_of_the_facts_has_the_answer_the_rules_give() {
        let answers = [
            None,
            Some(MemoryChoice::On),
            Some(MemoryChoice::Off),
            Some(MemoryChoice::Never),
        ];
        let mut asked = 0;
        for bits in 0..32u8 {
            for answered in answers {
                let facts = Facts {
                    local: bits & 1 != 0,
                    explicit: bits & 2 != 0,
                    setting_on: bits & 4 != 0,
                    asked_this_run: bits & 8 != 0,
                    block_present: bits & 16 != 0,
                    answered,
                };
                let expected = if !facts.local || !facts.explicit {
                    // Another machine's, or found by Leon itself: silence,
                    // whatever else is true.
                    Decision::Nothing
                } else if facts.block_present {
                    // Nothing needs writing: said, never asked, even with the
                    // setting off or after "never".
                    Decision::AlreadyOn
                } else if facts.setting_on && answered.is_none() && !facts.asked_this_run {
                    Decision::Ask
                } else {
                    Decision::Nothing
                };
                assert_eq!(decide(&facts), expected, "{facts:?}");
                asked += usize::from(expected == Decision::Ask);
            }
        }
        assert_eq!(asked, 1, "one combination of 128 asks");
    }

    fn offer(found: &[(&'static str, Found)]) -> Offer {
        Offer::new(
            ProjectId::from_string("p1"),
            "api".into(),
            "/srv/api".into(),
            &files::enable(found),
        )
    }

    fn file(text: &str) -> Found {
        Found::File(text.to_owned())
    }

    #[test]
    fn the_words_say_which_files_change_for_this_project() {
        // Two files that are there.
        let two = offer(&[
            ("AGENTS.md", file("# Agents\n")),
            ("CLAUDE.md", file("# Claude\n")),
            ("GEMINI.md", Found::Missing),
        ]);
        assert_eq!(two.title(), "Turn on agent memory for api?");
        assert_eq!(
            two.change(),
            "Leon adds a short section to AGENTS.md and CLAUDE.md in /srv/api. \
             Turn off agent memory for this project removes it again, byte for byte."
        );
        // None: one is created.
        let none = offer(&[
            ("AGENTS.md", Found::Missing),
            ("CLAUDE.md", Found::Missing),
            ("GEMINI.md", Found::Missing),
        ]);
        assert_eq!((none.writes.len(), none.creates), (0, Some("AGENTS.md")));
        assert_eq!(
            none.change(),
            "Leon creates AGENTS.md in /srv/api with a short section that tells agents about the memory. \
             Turn off agent memory for this project removes it again, byte for byte."
        );
        // One, and a file that only pulls it in is left alone and not named.
        let one = offer(&[
            ("AGENTS.md", file("# Agents\n")),
            ("CLAUDE.md", file("@AGENTS.md\n")),
            ("GEMINI.md", Found::Missing),
        ]);
        assert_eq!(
            one.change(),
            "Leon adds a short section to AGENTS.md in /srv/api. \
             Turn off agent memory for this project removes it again, byte for byte."
        );
        // All three.
        let three = offer(&[
            ("AGENTS.md", file("a\n")),
            ("CLAUDE.md", file("c\n")),
            ("GEMINI.md", file("g\n")),
        ]);
        assert!(three
            .change()
            .starts_with("Leon adds a short section to AGENTS.md, CLAUDE.md and GEMINI.md in "));
        assert!(two.writes_something() && none.writes_something());
        // Only a link Leon does not follow: nothing would be written.
        let nothing = offer(&[
            ("AGENTS.md", Found::Unusable("is a link".into())),
            ("CLAUDE.md", Found::Missing),
            ("GEMINI.md", Found::Missing),
        ]);
        assert!(!nothing.writes_something());
    }

    #[test]
    fn the_benefits_are_three_short_lines() {
        assert_eq!(BENEFITS.len(), 3);
        assert!(BENEFITS.iter().all(|line| line.len() < 100));
        assert!(BENEFITS[0].contains("Claude Code, Codex, opencode"));
        assert!(BENEFITS[1].contains("on this computer"));
        assert!(BENEFITS[2].contains("search"));
    }

    #[test]
    fn four_keys_answer_and_no_other() {
        assert_eq!(choice_of("enter"), Some(Choice::TurnOn));
        assert_eq!(choice_of("escape"), Some(Choice::NotNow));
        assert_eq!(choice_of("n"), Some(Choice::Never));
        assert_eq!(choice_of("d"), Some(Choice::StopAsking));
        for key in ["y", "space", "tab", "N", "q", "a", ""] {
            assert_eq!(choice_of(key), None, "{key:?}");
        }
        // Every choice has a button and a key that is its own.
        let keys: Vec<&str> = Choice::ALL.iter().map(|c| c.key_label()).collect();
        assert_eq!(keys, ["Enter", "Esc", "N", "D"]);
        for choice in Choice::ALL {
            assert_eq!(
                choice_of(&choice.key_label().to_lowercase().replace("esc", "escape")),
                Some(choice)
            );
            assert!(!choice.label().is_empty());
        }
    }

    #[test]
    fn the_status_lines_name_the_project() {
        assert_eq!(already_on("api"), "Agent memory is already on for api.");
        assert!(never_line("api").contains("for api again"));
        assert!(STOPPED_LINE.contains("Settings, Agents"));
    }
}
