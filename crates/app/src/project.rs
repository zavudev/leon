//! The project file: `leon.toml` at a project's root.
//!
//! A repository can carry the commands its people run all day and the one
//! that makes a fresh worktree usable, so Leon offers them where the work is:
//!
//! ```toml
//! [worktree]
//! setup = "npm install"          # run once in every new worktree
//!
//! [[script]]
//! name = "Test"                  # what the palette lists
//! command = "npm test"           # typed into a new terminal tab
//! icon = "terminal"              # optional, one of `icons::SCRIPT_ICONS`
//! key = "mod+shift+u"            # optional shortcut (see below)
//! ```
//!
//! [`parse`] turns the text into a [`ProjectFile`], or into a [`ProjectError`]
//! that names the line: a syntax error, an unknown field, a missing or empty
//! value, a name used twice, a key that is not one. It is a pure function; the
//! engine reads the file through the machine's runner (so it works over SSH and
//! the relay) and keeps the outcome as a [`ProjectState`].
//!
//! The file is checked into the repository, so what is in it was written by
//! whoever pushed. Two rules follow. A command is one line without control
//! characters (a carriage return or an escape sequence typed into a terminal is
//! not what the prompt that shows the command would show), and nothing in it
//! runs before [`crate::trust`] has been asked.
//!
//! **Keys.** A script's `key` is `mod`, `shift`, optionally `alt`, and one key
//! from `a` to `z`, `0` to `9` or `f1` to `f12`, joined by `+` (`mod` is Cmd on
//! macOS and Ctrl elsewhere; `cmd` and `ctrl` are accepted for it). Shift is
//! required on every platform: it is what keeps the chord Leon's while a
//! terminal has the keyboard. [`ScriptKey::chord`] builds the registry's own
//! [`Chord`]; [`ScriptKey::collision`] says which command of
//! [`keys::BINDINGS`] already has it. A script whose key collides is listed and
//! runs from the palette, but it gets no shortcut: it never shadows a command.

use crate::keys::{self, Chord, Command, Only};
use serde::Deserialize;
use std::sync::Arc;

/// The file's name at the project root.
pub const FILE_NAME: &str = "leon.toml";

/// The most scripts a file may list.
pub const MAX_SCRIPTS: usize = 40;

/// The longest script name, in characters.
pub const MAX_NAME: usize = 48;

/// The longest command, in characters.
pub const MAX_COMMAND: usize = 2000;

/// The keys a script can be given, as the registry spells them.
const KEYS: [&str; 48] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "f1",
    "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
];

/// What is wrong with the file, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectError {
    /// The line the problem is on, from 1; 0 when it is about the whole file.
    pub line: usize,
    /// What is wrong, in a sentence.
    pub message: String,
}

impl ProjectError {
    fn at(text: &str, offset: usize, message: impl Into<String>) -> Self {
        Self {
            line: line_of(text, offset),
            message: message.into(),
        }
    }

    fn whole(message: impl Into<String>) -> Self {
        Self {
            line: 0,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            0 => write!(f, "{FILE_NAME}: {}", self.message),
            line => write!(f, "{FILE_NAME} line {line}: {}", self.message),
        }
    }
}

impl std::error::Error for ProjectError {}

/// The line (from 1) a byte offset of `text` is on.
fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// A shortcut for a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptKey {
    key: &'static str,
    alt: bool,
}

impl ScriptKey {
    /// Reads `mod+shift+u`: `mod` (or `cmd`, `ctrl`), `shift`, optionally
    /// `alt`, and a key; in any order, case ignored.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (mut secondary, mut shift, mut alt, mut key) = (false, false, false, None);
        for part in text.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "mod" | "cmd" | "ctrl" | "secondary" => secondary = true,
                "shift" => shift = true,
                "alt" | "option" | "opt" => alt = true,
                "" => return Err(format!("{text:?} has an empty part.")),
                other => {
                    let Some(found) = KEYS.iter().find(|candidate| **candidate == other) else {
                        return Err(format!(
                            "{other:?} is not a key a script can have: use a letter, a digit or f1 to f12."
                        ));
                    };
                    if key.replace(*found).is_some() {
                        return Err(format!("{text:?} names more than one key."));
                    }
                }
            }
        }
        let Some(key) = key else {
            return Err(format!("{text:?} names no key."));
        };
        if !secondary || !shift {
            return Err(format!(
                "{text:?} must hold mod and shift: that is what keeps a shortcut Leon's while a terminal has the keyboard."
            ));
        }
        Ok(Self { key, alt })
    }

    /// The chord it is, in the registry's own terms.
    pub fn chord(&self) -> Chord {
        Chord {
            key: self.key,
            secondary: true,
            shift: true,
            alt: self.alt,
            control: false,
            only: Only::Any,
        }
    }

    /// The command of the registry that already has this chord, on any
    /// platform.
    pub fn collision(&self) -> Option<Command> {
        let mine = self.chord();
        keys::BINDINGS
            .iter()
            .find(|binding| {
                binding.chords.iter().any(|other| {
                    other.key == mine.key
                        && other.secondary == mine.secondary
                        && other.shift == mine.shift
                        && other.alt == mine.alt
                        && !other.control
                })
            })
            .map(|binding| binding.command)
    }

    /// The chord as the platform writes it.
    pub fn label(&self, mac: bool) -> String {
        self.chord().label_for(mac)
    }
}

/// One entry of `[[script]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// What the palette lists.
    pub name: String,
    /// The command line typed into a new terminal.
    pub command: String,
    /// One of [`crate::icons::SCRIPT_ICONS`], when given.
    pub icon: Option<String>,
    /// Its shortcut, when given.
    pub key: Option<ScriptKey>,
    /// The line of the file the entry starts on.
    pub line: usize,
    /// The line its `key` is on, when it has one.
    pub key_line: usize,
}

impl Script {
    /// The command of the registry whose chord this script's key is, when it
    /// is: the key is then not bound.
    pub fn collision(&self) -> Option<Command> {
        self.key.and_then(|key| key.collision())
    }

    /// Whether the script's key works: it has one and no command has it.
    pub fn bound(&self) -> Option<ScriptKey> {
        self.key.filter(|key| key.collision().is_none())
    }
}

/// The command run once in every new worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    /// The command line.
    pub command: String,
    /// The line of the file it is on.
    pub line: usize,
}

/// What `leon.toml` says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectFile {
    /// The scripts, in the file's order.
    pub scripts: Vec<Script>,
    /// The worktree setup, when there is one.
    pub setup: Option<Setup>,
}

impl ProjectFile {
    /// The script whose working key `stroke` is. A key that collides with a
    /// command of the registry is never answered.
    pub fn script_for(&self, stroke: &gpui_kit::Keystroke, mac: bool) -> Option<&Script> {
        self.scripts.iter().find(|script| {
            script
                .bound()
                .is_some_and(|key| key.chord().matches_on(stroke, mac))
        })
    }

    /// What is worth telling about the file although it parsed: each key that
    /// is not bound because a command of Leon has it.
    pub fn notices(&self, mac: bool) -> Vec<String> {
        self.scripts
            .iter()
            .filter_map(|script| {
                let key = script.key?;
                let command = key.collision()?;
                Some(format!(
                    "{FILE_NAME} line {}: {} is Leon's \"{}\", so the script \"{}\" has no shortcut; it still runs from the palette.",
                    script.key_line,
                    key.label(mac),
                    keys::label(command),
                    script.name
                ))
            })
            .collect()
    }
}

/// What the engine knows of a project's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectState {
    /// There is no `leon.toml`.
    Absent,
    /// It is there and wrong; nothing in it runs.
    Invalid(ProjectError),
    /// It is there and right.
    Loaded(Arc<ProjectFile>),
}

impl ProjectState {
    /// The file, when it is usable.
    pub fn file(&self) -> Option<&ProjectFile> {
        match self {
            Self::Loaded(file) => Some(file),
            Self::Absent | Self::Invalid(_) => None,
        }
    }
}

// ----- parsing -------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    script: Vec<RawScript>,
    worktree: Option<RawWorktree>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScript {
    name: toml::Spanned<String>,
    command: toml::Spanned<String>,
    icon: Option<toml::Spanned<String>>,
    key: Option<toml::Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorktree {
    setup: Option<toml::Spanned<String>>,
}

/// Reads the text of a `leon.toml`.
pub fn parse(text: &str) -> Result<ProjectFile, ProjectError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let raw: RawFile = toml::from_str(text).map_err(|error| match error.span() {
        Some(span) => ProjectError::at(text, span.start, error.message()),
        None => ProjectError::whole(error.message()),
    })?;
    if raw.script.len() > MAX_SCRIPTS {
        return Err(ProjectError::whole(format!(
            "A project lists at most {MAX_SCRIPTS} scripts."
        )));
    }
    let mut scripts: Vec<Script> = Vec::with_capacity(raw.script.len());
    for entry in raw.script {
        let line = line_of(text, entry.name.span().start);
        let name = entry.name.get_ref().trim().to_owned();
        if name.is_empty() {
            return Err(ProjectError::at(
                text,
                entry.name.span().start,
                "A script needs a name.",
            ));
        }
        if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
            return Err(ProjectError::at(
                text,
                entry.name.span().start,
                format!("A script name is one line of at most {MAX_NAME} characters."),
            ));
        }
        if let Some(first) = scripts
            .iter()
            .find(|other| other.name.eq_ignore_ascii_case(&name))
        {
            return Err(ProjectError::at(
                text,
                entry.name.span().start,
                format!(
                    "The name \"{name}\" is used twice (the first is on line {}).",
                    first.line
                ),
            ));
        }
        let command = checked_command(text, &entry.command)?;
        let icon = match &entry.icon {
            None => None,
            Some(icon) => {
                let given = icon.get_ref().trim().to_ascii_lowercase();
                if !crate::icons::SCRIPT_ICONS
                    .iter()
                    .any(|(known, _)| *known == given)
                {
                    return Err(ProjectError::at(
                        text,
                        icon.span().start,
                        format!(
                            "{given:?} is not an icon a script can have (one of: {}).",
                            crate::icons::SCRIPT_ICONS
                                .iter()
                                .map(|(name, _)| *name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                Some(given)
            }
        };
        let key_line = entry
            .key
            .as_ref()
            .map_or(0, |key| line_of(text, key.span().start));
        let key = match &entry.key {
            None => None,
            Some(key) => {
                let parsed = ScriptKey::parse(key.get_ref())
                    .map_err(|why| ProjectError::at(text, key.span().start, why))?;
                if let Some(other) = scripts.iter().find(|other| other.key == Some(parsed)) {
                    return Err(ProjectError {
                        line: key_line,
                        message: format!(
                            "The key {:?} is the one of \"{}\" (line {}) too.",
                            key.get_ref(),
                            other.name,
                            other.line
                        ),
                    });
                }
                Some(parsed)
            }
        };
        scripts.push(Script {
            name,
            command,
            icon,
            key,
            line,
            key_line,
        });
    }
    let setup = match raw.worktree.and_then(|worktree| worktree.setup) {
        None => None,
        Some(setup) => Some(Setup {
            command: checked_command(text, &setup)?,
            line: line_of(text, setup.span().start),
        }),
    };
    Ok(ProjectFile { scripts, setup })
}

/// A command as it is typed into a terminal: trimmed, not empty, one line,
/// no control characters.
fn checked_command(text: &str, value: &toml::Spanned<String>) -> Result<String, ProjectError> {
    let at = value.span().start;
    let command = value.get_ref().trim();
    if command.is_empty() {
        return Err(ProjectError::at(text, at, "A command cannot be empty."));
    }
    if command.contains(['\n', '\r']) {
        return Err(ProjectError::at(
            text,
            at,
            "A command is one line: join several with && (or ;), or put them in a script file of the repository.",
        ));
    }
    if command.chars().any(char::is_control) {
        return Err(ProjectError::at(
            text,
            at,
            "A command cannot contain control characters.",
        ));
    }
    if command.chars().count() > MAX_COMMAND {
        return Err(ProjectError::at(
            text,
            at,
            format!("A command is at most {MAX_COMMAND} characters."),
        ));
    }
    Ok(command.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(text: &str) -> ProjectError {
        parse(text).expect_err("the file is wrong")
    }

    #[test]
    fn an_empty_file_has_no_scripts_and_no_setup() {
        assert_eq!(parse("").unwrap(), ProjectFile::default());
        assert_eq!(
            parse("\u{feff}# only a comment\n").unwrap(),
            ProjectFile::default()
        );
    }

    #[test]
    fn scripts_and_setup_are_read_in_order() {
        let file = parse(
            "[worktree]\nsetup = \" npm install \"\n\n[[script]]\nname = \"Test\"\ncommand = \"npm test\"\nicon = \"terminal\"\nkey = \"Mod+Shift+U\"\n\n[[script]]\nname = \"Lint\"\ncommand = \"npm run lint\"\n",
        )
        .unwrap();
        assert_eq!(
            file.setup,
            Some(Setup {
                command: "npm install".into(),
                line: 2
            })
        );
        assert_eq!(file.scripts.len(), 2);
        let test = &file.scripts[0];
        assert_eq!(
            (test.name.as_str(), test.command.as_str(), test.line),
            ("Test", "npm test", 5)
        );
        assert_eq!(test.icon.as_deref(), Some("terminal"));
        assert_eq!(test.key, Some(ScriptKey::parse("mod+shift+u").unwrap()));
        assert_eq!(file.scripts[1].name, "Lint");
        assert_eq!(file.scripts[1].icon, None);
        assert_eq!(file.scripts[1].key, None);
    }

    #[test]
    fn a_syntax_error_names_its_line() {
        let wrong = error("[[script]]\nname = \"a\"\ncommand = \n");
        assert_eq!(wrong.line, 3, "{wrong}");
        assert!(wrong.to_string().starts_with("leon.toml line 3: "));
    }

    #[test]
    fn an_unknown_field_is_refused_where_it_is() {
        let wrong = error("[[script]]\nname = \"a\"\ncommand = \"b\"\nrun = \"c\"\n");
        assert_eq!(wrong.line, 4, "{wrong}");
        assert!(wrong.message.contains("run"), "{wrong}");
        assert_eq!(error("[project]\nname = \"x\"\n").line, 1);
        assert_eq!(error("[worktree]\nsetup = \"a\"\nafter = \"b\"\n").line, 3);
    }

    #[test]
    fn a_missing_command_names_the_entry() {
        let wrong =
            error("[[script]]\nname = \"a\"\ncommand = \"b\"\n\n[[script]]\nname = \"c\"\n");
        assert!(wrong.message.contains("command"), "{wrong}");
        assert!((5..=6).contains(&wrong.line), "{wrong}");
    }

    #[test]
    fn a_value_of_the_wrong_type_names_its_line() {
        let wrong = error("[[script]]\nname = \"a\"\ncommand = 3\n");
        assert_eq!(wrong.line, 3, "{wrong}");
    }

    #[test]
    fn names_and_commands_are_checked() {
        assert_eq!(
            error("[[script]]\nname = \"  \"\ncommand = \"b\"\n").line,
            2
        );
        assert_eq!(error("[[script]]\nname = \"a\"\ncommand = \" \"\n").line, 3);
        let long = "x".repeat(MAX_NAME + 1);
        assert_eq!(
            error(&format!("[[script]]\nname = \"{long}\"\ncommand = \"b\"\n")).line,
            2
        );
        let too_long = "x".repeat(MAX_COMMAND + 1);
        assert_eq!(
            error(&format!(
                "[[script]]\nname = \"a\"\ncommand = \"{too_long}\"\n"
            ))
            .line,
            3
        );
    }

    #[test]
    fn a_name_used_twice_is_refused_on_the_second() {
        let wrong = error(
            "[[script]]\nname = \"Test\"\ncommand = \"a\"\n\n[[script]]\nname = \"test\"\ncommand = \"b\"\n",
        );
        assert_eq!(wrong.line, 6);
        assert!(wrong.message.contains("line 2"), "{wrong}");
    }

    #[test]
    fn a_command_is_one_line_without_control_characters() {
        // TOML's own escapes can write them, and a repository can carry them.
        let newline = error("[[script]]\nname = \"a\"\ncommand = \"one\\ntwo\"\n");
        assert_eq!(newline.line, 3);
        assert!(newline.message.contains("&&"), "{newline}");
        let multi = error("[worktree]\nsetup = \"\"\"\nnpm install\nnpm test\n\"\"\"\n");
        assert_eq!(multi.line, 2, "{multi}");
        for sneaky in [
            "one\\rtwo",
            "a\\u001b[2Jb",
            "a\\u0003b",
            "a\\u0000b",
            "a\\u007fb",
        ] {
            let wrong = error(&format!(
                "[[script]]\nname = \"a\"\ncommand = \"{sneaky}\"\n"
            ));
            assert_eq!(wrong.line, 3, "{sneaky}: {wrong}");
        }
        assert!(
            parse("[worktree]\nsetup = \"a\\tb\"\n").is_err(),
            "a tab is a control character"
        );
    }

    #[test]
    fn a_trailing_newline_of_a_multiline_string_is_trimmed() {
        let file = parse("[worktree]\nsetup = \"\"\"\nnpm install\n\"\"\"\n").unwrap();
        assert_eq!(file.setup.unwrap().command, "npm install");
    }

    #[test]
    fn an_icon_must_be_one_of_the_offered() {
        let wrong = error("[[script]]\nname = \"a\"\ncommand = \"b\"\nicon = \"rocket-ship\"\n");
        assert_eq!(wrong.line, 4);
        assert!(wrong.message.contains("terminal"), "{wrong}");
        assert!(parse("[[script]]\nname = \"a\"\ncommand = \"b\"\nicon = \"Terminal\"\n").is_ok());
    }

    #[test]
    fn a_script_can_have_at_most_so_many() {
        let many: String = (0..=MAX_SCRIPTS)
            .map(|n| format!("[[script]]\nname = \"s{n}\"\ncommand = \"c\"\n"))
            .collect();
        assert_eq!(error(&many).line, 0);
    }

    #[test]
    fn keys_are_parsed_in_any_order_and_case() {
        let expected = ScriptKey::parse("mod+shift+k").unwrap();
        for same in [
            "shift+mod+k",
            "Ctrl+Shift+K",
            "cmd+shift+k",
            " mod + shift + k ",
        ] {
            assert_eq!(ScriptKey::parse(same), Ok(expected), "{same}");
        }
        let alt = ScriptKey::parse("mod+shift+alt+f5").unwrap();
        assert!(alt.chord().alt);
        assert_eq!(alt.chord().key, "f5");
    }

    #[test]
    fn keys_that_cannot_be_used_are_refused_with_the_reason() {
        for (text, mentions) in [
            ("k", "mod and shift"),
            ("mod+k", "mod and shift"),
            ("shift+k", "mod and shift"),
            ("mod+shift", "no key"),
            ("mod+shift+k+j", "more than one"),
            ("mod+shift+enter", "not a key"),
            ("mod+shift+", "empty part"),
            ("mod+shift+f13", "not a key"),
            ("", "empty part"),
        ] {
            let why = ScriptKey::parse(text).expect_err(text);
            assert!(why.contains(mentions), "{text:?}: {why}");
        }
    }

    #[test]
    fn a_key_given_to_two_scripts_is_refused_on_the_second() {
        let wrong = error(
            "[[script]]\nname = \"a\"\ncommand = \"x\"\nkey = \"mod+shift+u\"\n\n[[script]]\nname = \"b\"\ncommand = \"y\"\nkey = \"shift+ctrl+u\"\n",
        );
        assert_eq!(wrong.line, 9);
        assert!(wrong.message.contains("\"a\""), "{wrong}");
    }

    #[test]
    fn a_key_of_the_registry_is_a_collision_and_never_bound() {
        // Every chord of the registry that a script could ask for is found,
        // whatever command has it.
        let mut found = 0;
        for key in KEYS {
            for alt in [false, true] {
                let mine = ScriptKey { key, alt };
                let taken = keys::BINDINGS.iter().find(|binding| {
                    binding.chords.iter().any(|chord| {
                        chord.key == key
                            && chord.secondary
                            && chord.shift
                            && chord.alt == alt
                            && !chord.control
                    })
                });
                assert_eq!(mine.collision(), taken.map(|binding| binding.command));
                found += usize::from(taken.is_some());
            }
        }
        assert!(found > 0, "the registry has chords of this shape");
        // The shortcut sheet's own key is one of them.
        let taken = ScriptKey::parse("mod+shift+t").unwrap();
        assert_eq!(taken.collision(), Some(Command::OpenShell));
    }

    #[test]
    fn a_colliding_key_is_reported_and_answers_nothing() {
        let file = parse(
            "[[script]]\nname = \"shell\"\ncommand = \"x\"\nkey = \"mod+shift+t\"\n\n[[script]]\nname = \"free\"\ncommand = \"y\"\nkey = \"mod+shift+alt+f9\"\n",
        )
        .unwrap();
        assert_eq!(file.scripts[0].bound(), None);
        assert!(file.scripts[1].bound().is_some());
        let notices = file.notices(false);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("line 4"), "{notices:?}");
        assert!(notices[0].contains("Ctrl+Shift+T"), "{notices:?}");
        assert!(notices[0].contains("\"shell\""), "{notices:?}");
        let mac = file.notices(true);
        assert!(mac[0].contains('⌘'), "{mac:?}");

        let stroke = |source: &str| gpui_kit::Keystroke::parse(source).unwrap();
        // Off macOS the secondary key is Ctrl.
        assert_eq!(
            file.script_for(&stroke("ctrl-shift-alt-f9"), false)
                .map(|script| script.name.as_str()),
            Some("free")
        );
        assert_eq!(file.script_for(&stroke("ctrl-shift-t"), false), None);
        assert_eq!(
            file.script_for(&stroke("cmd-shift-alt-f9"), true)
                .map(|s| s.line),
            Some(7)
        );
        assert_eq!(file.script_for(&stroke("ctrl-shift-alt-f9"), true), None);
    }

    #[test]
    fn every_script_icon_is_listed_once() {
        let mut names: Vec<_> = crate::icons::SCRIPT_ICONS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), crate::icons::SCRIPT_ICONS.len());
    }
}
