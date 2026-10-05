//! The command line: two options and `--help`.
//!
//! Parsed by hand: the whole grammar is `--data-dir <path>`,
//! `--theme <light|dark|system>` (the appearance) and `--theme-name <id>` (which
//! theme: `leon`, `zavu`), each also accepted as
//! `--name=value`, and
//! pulling in a parser for that would cost more than it saves.
//!
//! One hidden option is for developers and bug reports, not for users:
//! `--diagnose terminal [--input <text>] [--timeout <seconds>] [-- <program>
//! [args...]]` runs a program in a pseudo-terminal without a window and
//! prints what its screen shows and how it ended (see `diagnose.rs`).
//! `--diagnose resume [--agent <claude|codex|opencode>]` prints what opening
//! the newest local history session of that agent would start, without
//! starting it.
//! `--diagnose connect <user@host[:port]>` runs the checklist of "Connect a
//! machine" from the command line and prints each check with its diagnosis;
//! it only attempts an SSH connection in batch mode (see `diagnose.rs`).
//! `--diagnose sessions-elsewhere` runs the real detection of sessions that
//! run in another terminal on this computer and prints, for each agent
//! process, what it found (see `diagnose.rs`).
//! `--diagnose usage [--network <claude|opencode>]...` runs the real collection
//! of the agents' usage limits for this computer and prints, per agent, the
//! source, the windows, how fresh they are or why they are unknown, and nothing
//! that identifies an account. A source that needs a credential and a network
//! call runs only when named with `--network`.

use crate::product;
use crate::settings::AppearanceChoice;
use crate::theme::ThemeId;
use leon_core::AgentKind;
use std::path::PathBuf;

/// What was asked for on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open the window.
    Run(Options),
    /// Print the usage and exit.
    Help,
    /// Print the version and exit.
    Version,
    /// Run a program in a terminal without a window and report on it.
    Diagnose(Diagnose),
    /// Ask for the system's folder dialog the way "Open project" does and
    /// report that the call was reached.
    DiagnoseFolderDialog {
        /// How long to leave the dialog up before ending the run.
        timeout: u64,
    },
    /// Print what resuming a real local history session of an agent would
    /// start: the program, the folder and the line typed into the shell.
    DiagnoseResume {
        /// Whose history to take the session from.
        agent: AgentKind,
    },
    /// Run the real detection of sessions running in another terminal on this
    /// computer and print what it found.
    DiagnoseElsewhere,
    /// Collect the usage limits of this computer and print them.
    DiagnoseUsage {
        /// The network sources switched on for this run.
        network: Vec<AgentKind>,
    },
    /// Run the checklist of "Connect a machine" against a destination and
    /// print each check with its diagnosis.
    DiagnoseConnect {
        /// `user@host[:port]`, as typed in the screen.
        destination: String,
    },
}

/// What the hidden `--diagnose terminal` run is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnose {
    /// The program and its arguments; a shell echo when empty.
    pub command: Vec<String>,
    /// Text typed into the terminal once the program has started, with `\n`,
    /// `\r`, `\t`, `\e` and `\xNN` written as such.
    pub input: Option<String>,
    /// How long to wait for the program to end, in seconds.
    pub timeout: u64,
}

/// The options of a normal run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Where the database and the settings live; the platform's directory
    /// when absent.
    pub data_dir: Option<PathBuf>,
    /// The appearance for this run, over the saved one.
    pub theme: Option<AppearanceChoice>,
    /// The theme for this run, over the saved one.
    pub theme_name: Option<ThemeId>,
}

/// The text printed by `--help`.
pub fn usage() -> String {
    format!(
        "{name} {version}\n\n\
         Usage: {slug} [options]\n\n\
         Options:\n  \
         --data-dir <path>              Keep the database and settings here\n  \
         --theme <light|dark|system>    Use this appearance for this run\n  \
         --theme-name <id>              Use this theme for this run: {names}, or a user theme's id\n  \
         -h, --help                     Print this help\n  \
         -V, --version                  Print the version",
        name = product::PRODUCT_NAME,
        version = product::VERSION,
        slug = product::SLUG,
        names = ThemeId::ALL.map(ThemeId::slug).join(", "),
    )
}

/// The ids of the themes on offer, built-in ones first, as a list for a message.
pub fn theme_ids() -> String {
    crate::theme::registry::usable()
        .into_iter()
        .map(ThemeId::slug)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The theme `--theme-name` named, now that the themes folder has been read:
/// a built-in or a user theme that loads. An error says what is on offer.
pub fn resolve_theme(asked: ThemeId) -> Result<ThemeId, String> {
    match ThemeId::parse(asked.slug()) {
        Some(id) => match id.problem() {
            Some(reason) => Err(format!("Theme {:?} is invalid: {reason}", id.slug())),
            None => Ok(id),
        },
        None => Err(format!(
            "Unknown theme {:?}: use {}.",
            asked.slug(),
            theme_ids()
        )),
    }
}

/// Parses the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut options = Options::default();
    let mut diagnose: Option<Diagnose> = None;
    let mut connect: Option<String> = None;
    let mut folder_dialog = false;
    let mut resume = false;
    let mut elsewhere = false;
    let mut usage = false;
    let mut network: Vec<AgentKind> = Vec::new();
    let mut agent = AgentKind::Claude;
    let mut dialog_timeout = 4u64;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_owned(), Some(value.to_owned()))
            }
            _ => (argument.clone(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{name} needs {what}."))
        };
        match name.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--data-dir" => {
                let path = value("a path")?;
                if path.is_empty() {
                    return Err("--data-dir needs a path.".to_owned());
                }
                options.data_dir = Some(PathBuf::from(path));
            }
            "--diagnose" => {
                let what = value("what to diagnose")?;
                if what == "folder-dialog" {
                    folder_dialog = true;
                    continue;
                }
                if what == "resume" {
                    resume = true;
                    continue;
                }
                if what == "sessions-elsewhere" {
                    elsewhere = true;
                    continue;
                }
                if what == "usage" {
                    usage = true;
                    continue;
                }
                if what == "connect" {
                    let destination = value("user@host[:port]")?;
                    connect = Some(destination);
                    continue;
                }
                if what != "terminal" {
                    return Err(format!(
                        "Cannot diagnose {what:?}: only \"terminal\", \"folder-dialog\", \"resume\", \"sessions-elsewhere\", \"usage\" and \"connect\"."
                    ));
                }
                diagnose = Some(Diagnose {
                    command: Vec::new(),
                    input: None,
                    timeout: 10,
                });
            }
            "--network" => {
                let text = value("claude or opencode")?;
                if !usage {
                    return Err("--network only goes with --diagnose usage.".to_owned());
                }
                match AgentKind::parse(&text) {
                    Some(agent @ (AgentKind::Claude | AgentKind::Opencode)) => network.push(agent),
                    _ => {
                        return Err(format!(
                            "{text:?} has no network source: use claude or opencode."
                        ))
                    }
                }
            }
            "--agent" => {
                let text = value("claude, codex or opencode")?;
                if !resume {
                    return Err("--agent only goes with --diagnose resume.".to_owned());
                }
                agent = AgentKind::parse(&text).ok_or_else(|| {
                    format!("Unknown agent {text:?}: use claude, codex or opencode.")
                })?;
            }
            "--input" => {
                let text = value("text to type")?;
                diagnose
                    .as_mut()
                    .ok_or("--input only goes with --diagnose.")?
                    .input = Some(text);
            }
            "--timeout" => {
                let text = value("a number of seconds")?;
                let seconds: u64 = text
                    .parse()
                    .map_err(|_| format!("--timeout needs a number of seconds, not {text:?}."))?;
                match diagnose.as_mut() {
                    Some(diagnose) => diagnose.timeout = seconds,
                    None if folder_dialog => dialog_timeout = seconds,
                    None => return Err("--timeout only goes with --diagnose.".to_owned()),
                }
            }
            "--" => {
                let rest: Vec<String> = args.by_ref().collect();
                diagnose
                    .as_mut()
                    .ok_or("A command only goes with --diagnose.")?
                    .command = rest;
            }
            "--theme" => {
                let text = value("light, dark or system")?;
                options.theme = Some(AppearanceChoice::parse(&text).ok_or_else(|| {
                    format!("Unknown appearance {text:?}: use light, dark or system.")
                })?);
            }
            "--theme-name" => {
                let text = value("a theme id")?;
                // Whether a theme of that id exists is known once the themes
                // folder is read: see [`resolve_theme`].
                if crate::theme::file_slug(&text).is_empty() {
                    return Err(format!("Unknown theme {text:?}: use {}.", theme_ids()));
                }
                options.theme_name = Some(crate::theme::registry::id_of(&text));
            }
            other => return Err(format!("Unknown option {other:?}.")),
        }
    }
    if usage {
        return Ok(Command::DiagnoseUsage { network });
    }
    if elsewhere {
        return Ok(Command::DiagnoseElsewhere);
    }
    if let Some(destination) = connect {
        return Ok(Command::DiagnoseConnect { destination });
    }
    if resume {
        return Ok(Command::DiagnoseResume { agent });
    }
    if folder_dialog {
        return Ok(Command::DiagnoseFolderDialog {
            timeout: dialog_timeout,
        });
    }
    match diagnose {
        Some(diagnose) => Ok(Command::Diagnose(diagnose)),
        None => Ok(Command::Run(options)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn diagnose_connect_takes_a_destination() {
        assert_eq!(
            parsed(&["--diagnose", "connect", "dev@box:2222"]),
            Ok(Command::DiagnoseConnect {
                destination: "dev@box:2222".into()
            })
        );
        assert!(parsed(&["--diagnose", "connect"]).is_err());
    }

    #[test]
    fn diagnose_usage_leaves_every_network_source_off_unless_named() {
        assert_eq!(
            parsed(&["--diagnose", "usage"]),
            Ok(Command::DiagnoseUsage { network: vec![] })
        );
        assert_eq!(
            parsed(&[
                "--diagnose",
                "usage",
                "--network",
                "claude",
                "--network=opencode"
            ]),
            Ok(Command::DiagnoseUsage {
                network: vec![AgentKind::Claude, AgentKind::Opencode]
            })
        );
        assert!(parsed(&["--diagnose", "usage", "--network", "codex"]).is_err());
        assert!(parsed(&["--network", "claude"]).is_err());
    }

    #[test]
    fn no_arguments_is_a_plain_run() {
        assert_eq!(parsed(&[]), Ok(Command::Run(Options::default())));
    }

    #[test]
    fn the_data_directory_is_taken_as_a_pair_or_with_an_equals_sign() {
        let expected = Ok(Command::Run(Options {
            data_dir: Some(PathBuf::from("/tmp/leon")),
            theme: None,
            theme_name: None,
        }));
        assert_eq!(parsed(&["--data-dir", "/tmp/leon"]), expected);
        assert_eq!(parsed(&["--data-dir=/tmp/leon"]), expected);
    }

    #[test]
    fn the_theme_is_read_in_any_case() {
        assert_eq!(
            parsed(&["--theme", "Light"]),
            Ok(Command::Run(Options {
                data_dir: None,
                theme: Some(AppearanceChoice::Light),
                theme_name: None,
            }))
        );
    }

    #[test]
    fn both_options_can_be_given_together() {
        assert_eq!(
            parsed(&["--theme=dark", "--data-dir", "d"]),
            Ok(Command::Run(Options {
                data_dir: Some(PathBuf::from("d")),
                theme: Some(AppearanceChoice::Dark),
                theme_name: None,
            }))
        );
    }

    #[test]
    fn the_theme_name_is_read_by_id_or_name_and_is_independent_of_the_appearance() {
        assert_eq!(
            parsed(&["--theme-name", "LEON", "--theme=light"]),
            Ok(Command::Run(Options {
                data_dir: None,
                theme: Some(AppearanceChoice::Light),
                theme_name: Some(ThemeId::Leon),
            }))
        );
        assert_eq!(
            parsed(&["--theme-name=zavu"]),
            Ok(Command::Run(Options {
                theme_name: Some(ThemeId::Zavu),
                ..Options::default()
            }))
        );
    }

    #[test]
    fn an_unknown_theme_name_is_an_error_that_lists_the_ids_once_the_themes_are_read() {
        // The id is kept as typed; whether a theme answers to it is decided
        // after the themes folder is read.
        let Ok(Command::Run(options)) = parsed(&["--theme-name", "solarized"]) else {
            panic!("the name is kept for later");
        };
        let error = resolve_theme(options.theme_name.unwrap()).unwrap_err();
        assert!(error.contains("solarized"));
        for id in ThemeId::ALL {
            assert!(error.contains(id.slug()), "{error}");
        }
        assert!(parsed(&["--theme-name"]).is_err());
        assert!(parsed(&["--theme-name", "  "]).is_err());
        assert_eq!(resolve_theme(ThemeId::Zavu), Ok(ThemeId::Zavu));
    }

    #[test]
    fn the_usage_documents_both_theme_options_and_every_id() {
        let text = usage();
        assert!(text.contains("--theme-name"));
        for id in ThemeId::ALL {
            assert!(text.contains(id.slug()), "{text}");
        }
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(parsed(&["-h"]), Ok(Command::Help));
        assert_eq!(parsed(&["--theme", "dark", "--help"]), Ok(Command::Help));
        assert_eq!(parsed(&["--version"]), Ok(Command::Version));
    }

    #[test]
    fn a_missing_value_is_an_error() {
        assert!(parsed(&["--data-dir"]).is_err());
        assert!(parsed(&["--data-dir="]).is_err());
        assert!(parsed(&["--theme"]).is_err());
    }

    #[test]
    fn an_unknown_option_or_theme_is_an_error_that_names_it() {
        assert!(parsed(&["--nope"]).unwrap_err().contains("--nope"));
        assert!(parsed(&["--theme", "solarized"])
            .unwrap_err()
            .contains("solarized"));
        assert!(parsed(&["stray"]).is_err());
    }

    #[test]
    fn the_usage_names_the_product_and_both_options() {
        let text = usage();
        assert!(text.contains("Leon"));
        assert!(text.contains("--data-dir"));
        assert!(text.contains("--theme"));
    }

    #[test]
    fn the_hidden_diagnostic_takes_a_command_after_two_dashes() {
        assert_eq!(
            parsed(&[
                "--diagnose",
                "terminal",
                "--input",
                "hi\\n",
                "--timeout",
                "3",
                "--",
                "claude",
                "--version"
            ]),
            Ok(Command::Diagnose(Diagnose {
                command: vec!["claude".into(), "--version".into()],
                input: Some("hi\\n".into()),
                timeout: 3,
            }))
        );
    }

    #[test]
    fn the_diagnostic_alone_has_defaults_and_is_not_in_the_usage() {
        assert_eq!(
            parsed(&["--diagnose=terminal"]),
            Ok(Command::Diagnose(Diagnose {
                command: Vec::new(),
                input: None,
                timeout: 10,
            }))
        );
        assert!(!usage().contains("diagnose"));
    }

    #[test]
    fn the_diagnostic_options_need_the_diagnostic() {
        assert!(parsed(&["--input", "x"]).is_err());
        assert!(parsed(&["--timeout", "3"]).is_err());
        assert!(parsed(&["--", "ls"]).is_err());
        assert!(parsed(&["--diagnose", "window"]).is_err());
        assert!(parsed(&["--diagnose", "terminal", "--timeout", "soon"]).is_err());
    }

    #[test]
    fn the_resume_diagnostic_is_hidden_and_takes_an_agent() {
        assert_eq!(
            parsed(&["--diagnose", "resume"]),
            Ok(Command::DiagnoseResume {
                agent: AgentKind::Claude
            })
        );
        assert_eq!(
            parsed(&["--diagnose=resume", "--agent", "codex"]),
            Ok(Command::DiagnoseResume {
                agent: AgentKind::Codex
            })
        );
        assert!(parsed(&["--diagnose", "resume", "--agent", "gemini"]).is_err());
        assert!(parsed(&["--agent", "claude"]).is_err());
        assert!(!usage().contains("resume"));
    }

    #[test]
    fn the_folder_dialog_diagnostic_is_hidden_and_takes_a_timeout() {
        assert_eq!(
            parsed(&["--diagnose", "folder-dialog"]),
            Ok(Command::DiagnoseFolderDialog { timeout: 4 })
        );
        assert_eq!(
            parsed(&["--diagnose=folder-dialog", "--timeout", "9"]),
            Ok(Command::DiagnoseFolderDialog { timeout: 9 })
        );
        assert!(!usage().contains("folder-dialog"));
    }
}
