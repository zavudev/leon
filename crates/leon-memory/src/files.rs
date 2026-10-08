//! Which instruction files a project has and which of them get the block.
//!
//! Agents read their standing instructions from a file at the project's root:
//! `AGENTS.md` (Codex, opencode and most others), `CLAUDE.md` (Claude Code),
//! `GEMINI.md` (Gemini). The rule for turning the memory on:
//!
//! * every instruction file that exists gets the block;
//! * when none exists, `AGENTS.md` is created and the others are not;
//! * a file that is only another name for one of the others (a symbolic link
//!   to it) is written once, through the other;
//! * a file that pulls another one in (`@AGENTS.md` on a line of its own, as
//!   `CLAUDE.md` often does) is left alone: the agent already reads the block
//!   through the file it pulls in;
//! * a link that leads anywhere else, or anything that is not a plain file, is
//!   never followed or written.
//!
//! Turning it off removes the block from every file that holds it. The file
//! Leon created says so in the block's first line: it is removed when the
//! block was still all it held. Every other file is left as it was before,
//! byte for byte, also one that was empty.
//!
//! The application looks at the disk and says what it [`Found`]; the decisions
//! here are pure.

use crate::block;

/// The instruction files, in the order they are looked at.
pub const NAMES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "GEMINI.md"];

/// The file created when a project has none.
pub const CREATED: &str = NAMES[0];

/// What is at an instruction file's place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// Nothing.
    Missing,
    /// A plain file inside the project, with its text.
    File(String),
    /// A link to another of the instruction files of the same project.
    Alias(&'static str),
    /// Something Leon does not write, and why.
    Unusable(String),
}

/// What to do with one instruction file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Replace its text.
    Write {
        /// The file.
        name: &'static str,
        /// Its new text.
        text: String,
    },
    /// Create it with this text.
    Create {
        /// The file.
        name: &'static str,
        /// Its text.
        text: String,
    },
    /// Remove it: the block was all it held.
    Delete {
        /// The file.
        name: &'static str,
    },
    /// Leave it as it is, and why.
    Leave {
        /// The file.
        name: &'static str,
        /// The reason, as a clause: "already has the block".
        why: String,
    },
}

impl Step {
    /// The file the step is about.
    pub fn name(&self) -> &'static str {
        match self {
            Step::Write { name, .. }
            | Step::Create { name, .. }
            | Step::Delete { name }
            | Step::Leave { name, .. } => name,
        }
    }

    /// Whether the step changes the disk.
    pub fn changes(&self) -> bool {
        !matches!(self, Step::Leave { .. })
    }
}

/// Whether a text pulls in the file called `name`: a line that is `@name` or
/// `@./name` and nothing else.
pub fn imports(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        line.trim()
            .strip_prefix('@')
            .map(|path| path.strip_prefix("./").unwrap_or(path))
            .is_some_and(|path| path == name)
    })
}

fn text_of<'a>(found: &'a [(&'static str, Found)], name: &str) -> Option<&'a str> {
    found.iter().find_map(|(other, found)| match found {
        Found::File(text) if *other == name => Some(text.as_str()),
        _ => None,
    })
}

/// The file `name` reads the block through, when it pulls one of the other
/// plain files in (and that one does not pull it in back).
fn imported(found: &[(&'static str, Found)], name: &str, text: &str) -> Option<&'static str> {
    found.iter().find_map(|(other, _)| {
        let theirs = text_of(found, other)?;
        (*other != name && imports(text, other) && !imports(theirs, name)).then_some(*other)
    })
}

/// What turning the memory on does to each instruction file. `found` holds
/// one entry per name of [`NAMES`].
pub fn enable(found: &[(&'static str, Found)]) -> Vec<Step> {
    let mut steps: Vec<Step> = found
        .iter()
        .filter_map(|(name, at)| {
            let name = *name;
            Some(match at {
                Found::Missing => return None,
                Found::Alias(other) => Step::Leave {
                    name,
                    why: format!("is a link to {other}"),
                },
                Found::Unusable(why) => Step::Leave {
                    name,
                    why: why.clone(),
                },
                Found::File(text) => match imported(found, name, text) {
                    // A block somebody put there by hand is kept current.
                    Some(other) if !block::present(text) => Step::Leave {
                        name,
                        why: format!("pulls in {other}"),
                    },
                    _ if block::current(text) => Step::Leave {
                        name,
                        why: "already has the block".to_owned(),
                    },
                    _ => Step::Write {
                        name,
                        text: block::insert(text),
                    },
                },
            })
        })
        .collect();
    // Nothing to hold the block: make the one file every agent can read. Not
    // when a link or an odd file is in the way of knowing what is there.
    let any = found.iter().any(|(_, at)| !matches!(at, Found::Missing));
    if !any {
        steps.push(Step::Create {
            name: CREATED,
            text: block::create(),
        });
    }
    steps
}

/// What turning the memory off does to each instruction file.
pub fn disable(found: &[(&'static str, Found)]) -> Vec<Step> {
    found
        .iter()
        .filter_map(|(name, at)| {
            let name = *name;
            match at {
                Found::File(text) if block::present(text) => Some(if block::created_alone(text) {
                    Step::Delete { name }
                } else {
                    Step::Write {
                        name,
                        text: block::remove(text),
                    }
                }),
                Found::Unusable(why) => Some(Step::Leave {
                    name,
                    why: why.clone(),
                }),
                _ => None,
            }
        })
        .collect()
}

/// The instruction files that hold the block, by name.
pub fn holding(found: &[(&'static str, Found)]) -> Vec<&'static str> {
    found
        .iter()
        .filter_map(|(name, at)| match at {
            Found::File(text) if block::present(text) => Some(*name),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(agents: Found, claude: Found, gemini: Found) -> Vec<(&'static str, Found)> {
        vec![
            ("AGENTS.md", agents),
            ("CLAUDE.md", claude),
            ("GEMINI.md", gemini),
        ]
    }

    fn file(text: &str) -> Found {
        Found::File(text.to_owned())
    }

    /// The disk after the steps.
    fn apply(before: &[(&'static str, Found)], steps: &[Step]) -> Vec<(&'static str, Found)> {
        before
            .iter()
            .map(|(name, at)| {
                let after = match steps.iter().find(|step| step.name() == *name) {
                    Some(Step::Write { text, .. } | Step::Create { text, .. }) => {
                        Found::File(text.clone())
                    }
                    Some(Step::Delete { .. }) => Found::Missing,
                    Some(Step::Leave { .. }) | None => at.clone(),
                };
                (*name, after)
            })
            .collect()
    }

    #[test]
    fn every_file_that_exists_gets_the_block_and_no_other_is_made() {
        let before = found(file("# Agents\n"), Found::Missing, file("# Gemini"));
        let steps = enable(&before);
        assert_eq!(steps.len(), 2);
        assert!(
            matches!(&steps[0], Step::Write { name: "AGENTS.md", text } if block::current(text))
        );
        assert!(
            matches!(&steps[1], Step::Write { name: "GEMINI.md", text } if block::current(text))
        );
        assert_eq!(holding(&apply(&before, &steps)), ["AGENTS.md", "GEMINI.md"]);
    }

    #[test]
    fn a_project_without_any_gets_agents_md_only() {
        let before = found(Found::Missing, Found::Missing, Found::Missing);
        let steps = enable(&before);
        assert_eq!(
            steps,
            [Step::Create {
                name: "AGENTS.md",
                text: block::create()
            }]
        );
    }

    #[test]
    fn a_link_between_two_of_them_is_written_once() {
        let before = found(
            file("# Agents\n"),
            Found::Alias("AGENTS.md"),
            Found::Missing,
        );
        let steps = enable(&before);
        assert!(matches!(
            &steps[0],
            Step::Write {
                name: "AGENTS.md",
                ..
            }
        ));
        assert_eq!(
            steps[1],
            Step::Leave {
                name: "CLAUDE.md",
                why: "is a link to AGENTS.md".into()
            }
        );
        // The reverse: AGENTS.md is the link.
        let before = found(
            Found::Alias("CLAUDE.md"),
            file("# Claude\n"),
            Found::Missing,
        );
        let steps = enable(&before);
        assert!(matches!(
            &steps[0],
            Step::Leave {
                name: "AGENTS.md",
                ..
            }
        ));
        assert!(matches!(
            &steps[1],
            Step::Write {
                name: "CLAUDE.md",
                ..
            }
        ));
    }

    #[test]
    fn a_file_that_pulls_another_in_is_left_alone() {
        let before = found(
            file("# Agents\n"),
            file("@AGENTS.md\n\nClaude only: be brief.\n"),
            Found::Missing,
        );
        let steps = enable(&before);
        assert!(matches!(
            &steps[0],
            Step::Write {
                name: "AGENTS.md",
                ..
            }
        ));
        assert_eq!(
            steps[1],
            Step::Leave {
                name: "CLAUDE.md",
                why: "pulls in AGENTS.md".into()
            }
        );
        assert!(imports("  @./AGENTS.md  \n", "AGENTS.md"));
        assert!(!imports("see @AGENTS.md for more\n", "AGENTS.md"));
        assert!(!imports("@docs/AGENTS.md\n", "AGENTS.md"));
        // Pulling in a file that is not there is not a reason.
        let alone = found(Found::Missing, file("@AGENTS.md\n"), Found::Missing);
        assert!(matches!(
            &enable(&alone)[0],
            Step::Write {
                name: "CLAUDE.md",
                ..
            }
        ));
        // Two files that pull each other in both get it.
        let circle = found(file("@CLAUDE.md\n"), file("@AGENTS.md\n"), Found::Missing);
        assert!(enable(&circle).iter().all(Step::changes));
    }

    #[test]
    fn something_leon_does_not_write_is_reported_and_blocks_nothing_else() {
        let before = found(
            Found::Unusable("is a link out of the project".into()),
            file("# Claude\n"),
            Found::Missing,
        );
        let steps = enable(&before);
        assert_eq!(
            steps[0],
            Step::Leave {
                name: "AGENTS.md",
                why: "is a link out of the project".into()
            }
        );
        assert!(matches!(
            &steps[1],
            Step::Write {
                name: "CLAUDE.md",
                ..
            }
        ));
        // It is the only thing there: nothing is created beside it.
        let only = found(
            Found::Unusable("is a folder".into()),
            Found::Missing,
            Found::Missing,
        );
        assert!(enable(&only).iter().all(|step| !step.changes()));
    }

    #[test]
    fn turning_it_on_twice_changes_nothing_the_second_time() {
        let before = found(file("# Agents\n"), file("# Claude\r\n"), Found::Missing);
        let after = apply(&before, &enable(&before));
        let again = enable(&after);
        assert!(again.iter().all(|step| !step.changes()), "{again:?}");
        assert_eq!(
            again[0],
            Step::Leave {
                name: "AGENTS.md",
                why: "already has the block".into()
            }
        );
    }

    #[test]
    fn turning_it_off_puts_every_file_back_as_it_was() {
        for before in [
            found(file("# Agents\n"), file("# Claude\r\nno newline"), file("")),
            found(Found::Missing, Found::Missing, Found::Missing),
            found(
                file("# Agents\n"),
                Found::Alias("AGENTS.md"),
                Found::Missing,
            ),
            found(file("# Agents\n"), file("@AGENTS.md\n"), Found::Missing),
        ] {
            let on = apply(&before, &enable(&before));
            let off = apply(&on, &disable(&on));
            // Exactly as before: an empty file is empty again, and the file
            // that was made is gone.
            assert_eq!(off, before);
            assert!(holding(&off).is_empty());
        }
    }

    #[test]
    fn turning_it_off_where_it_is_not_on_does_nothing() {
        let before = found(
            file("# Agents\n"),
            Found::Missing,
            Found::Alias("AGENTS.md"),
        );
        assert!(disable(&before).is_empty());
    }
}
