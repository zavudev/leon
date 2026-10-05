//! What the "Connect a machine" screen says and checks, without drawing it.
//!
//! The screen is a form plus an explanation. Everything that has a right
//! answer lives here so it can be tested without a window: how the host field
//! is split into user, host and port while it is typed, what is wrong with
//! the fields, the exact `ssh` and `ssh-copy-id` lines that follow from them,
//! and the per-platform "how do I…" text.

use leon_remote::connect::{KeyFile, Target};

use crate::address::{self, Destination};

/// The explanation at the top of the screen: what a machine is.
pub const WHAT_IT_IS: &str = "A machine is another computer (a server, a desktop at the office, \
a spare laptop) whose projects, worktrees and agent sessions you drive from here.";

/// How Leon reaches it.
pub const HOW_IT_REACHES: &str = "Leon reaches it with your system's SSH, signing in as you. \
Leon installs nothing on the other computer.";

/// Why three things must be true.
pub const THREE_THINGS: &str = "Three things must be true on the other computer:";

/// The operating system of the OTHER computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS.
    Mac,
    /// Linux.
    Linux,
    /// Windows: not supported yet.
    Windows,
}

impl Platform {
    /// The choices, in order.
    pub const ALL: [Platform; 3] = [Self::Mac, Self::Linux, Self::Windows];

    /// What the choice is called.
    pub fn label(self) -> &'static str {
        match self {
            Self::Mac => "macOS",
            Self::Linux => "Linux",
            Self::Windows => "Windows (not yet)",
        }
    }

    /// The platform after or before this one, stopping at the ends.
    pub fn step(self, forward: bool) -> Self {
        let at = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
        let next = if forward {
            at + 1
        } else {
            at.saturating_sub(1)
        };
        Self::ALL[next.min(Self::ALL.len() - 1)]
    }
}

/// One step of a "how do I…" answer: a sentence, and a command to run when
/// there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// What to do.
    pub text: String,
    /// The command, shown in mono.
    pub command: Option<String>,
}

fn line(text: &str) -> Line {
    Line {
        text: text.to_owned(),
        command: None,
    }
}

fn run(text: &str, command: &str) -> Line {
    Line {
        text: text.to_owned(),
        command: Some(command.to_owned()),
    }
}

/// One collapsible row of "How do I…?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HowTo {
    /// The requirement, as a sentence.
    pub title: &'static str,
    /// The question it answers, as the row says it.
    pub question: &'static str,
    /// The answer.
    pub lines: Vec<Line>,
}

/// The three rows for the other computer's platform. `target` fills the
/// `ssh-copy-id` line; `keys` are the public keys found in `~/.ssh`.
pub fn howtos(platform: Platform, target: &Target, keys: &[KeyFile]) -> [HowTo; 3] {
    let server = match platform {
        Platform::Mac => vec![
            line("On that Mac, open System Settings, General, Sharing."),
            line("Turn on Remote Login and allow your user."),
            run(
                "Or, in its Terminal:",
                "sudo systemsetup -setremotelogin on",
            ),
        ],
        Platform::Linux => vec![
            line("On that computer, install and start the OpenSSH server:"),
            run("Debian, Ubuntu:", "sudo apt install openssh-server"),
            run("Then switch it on:", "sudo systemctl enable --now ssh"),
            run("Fedora, Arch:", "sudo systemctl enable --now sshd"),
        ],
        Platform::Windows => vec![line(
            "Windows is not supported yet. Leon needs a macOS or Linux computer on the other end.",
        )],
    };
    let mut key = Vec::new();
    if keys.is_empty() {
        key.push(line(
            "This computer has no key in ~/.ssh yet. Create one first:",
        ));
        key.push(run("Make a key:", "ssh-keygen -t ed25519"));
    } else {
        let names: Vec<&str> = keys.iter().map(|key| key.public.as_str()).collect();
        key.push(line(&format!(
            "Keys found on this computer: {}.",
            names.join(", ")
        )));
    }
    key.push(run(
        "Then, in a terminal on THIS computer (it asks for the other computer's password once):",
        &target.copy_id_line(),
    ));
    let agent = vec![
        line("Install at least one coding agent on the other computer, as you did here."),
        line("Leon looks for these there:"),
        line(leon_remote::connect::install_hint("claude")),
        line(leon_remote::connect::install_hint("codex")),
        line(leon_remote::connect::install_hint("opencode")),
    ];
    [
        HowTo {
            title: "SSH is switched on",
            question: "How do I switch on SSH there?",
            lines: server,
        },
        HowTo {
            title: "Your key is allowed in",
            question: "How do I let my key in?",
            lines: key,
        },
        HowTo {
            title: "An agent is installed",
            question: "Which agents does Leon look for?",
            lines: agent,
        },
    ]
}

/// The fields of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// What the machine is called.
    Name,
    /// Host, with or without user and port.
    Host,
    /// Login name.
    User,
    /// TCP port.
    Port,
    /// Private key file.
    Identity,
    /// Folder to look for projects in.
    Folder,
}

impl Field {
    /// Every field, in tab order.
    pub const ALL: [Field; 6] = [
        Self::Name,
        Self::Host,
        Self::User,
        Self::Port,
        Self::Identity,
        Self::Folder,
    ];

    /// What the field is called.
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Host => "Host",
            Self::User => "User",
            Self::Port => "Port",
            Self::Identity => "Identity file",
            Self::Folder => "Start folder",
        }
    }

    /// What the empty field says, other than the user's default.
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Name => "Build server",
            Self::Host => "build.example, user@host:2222, ssh://…",
            Self::User => "",
            Self::Port => "22",
            Self::Identity => "optional: ~/.ssh/id_ed25519",
            Self::Folder => "optional: /home/dev/code",
        }
    }
}

/// What is wrong with a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The field.
    pub field: Field,
    /// What to change, in plain words.
    pub message: String,
}

/// The values of the fields, as typed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Form {
    /// Name.
    pub name: String,
    /// Host (after splitting: the host alone).
    pub host: String,
    /// User.
    pub user: String,
    /// Port.
    pub port: String,
    /// Identity file.
    pub identity: String,
    /// Start folder.
    pub folder: String,
}

/// The name of the person using this computer, which `ssh` signs in as when
/// no user is given.
pub fn login_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "you".to_owned())
}

/// When the host field holds more than a host (`user@host`, `host:port`,
/// `ssh://…`) and parses, its parts; `None` while it is a plain host or does
/// not parse yet (half-typed `dev@` stays as it is).
pub fn split_host(text: &str) -> Option<Destination> {
    let text = text.trim();
    let compound = text.contains('@') || text.starts_with("ssh://") || has_port(text);
    if !compound {
        return None;
    }
    address::parse_destination(text).ok()
}

/// Whether `text` ends in `:digits` after a single colon, or after a bracket.
fn has_port(text: &str) -> bool {
    if text.starts_with('[') {
        return text.contains("]:");
    }
    text.matches(':').count() == 1 && !text.ends_with(':')
}

impl Form {
    /// The form of an existing machine.
    pub fn of(name: &str, target: &Target) -> Self {
        Self {
            name: name.to_owned(),
            host: target.host.clone(),
            user: target.user.clone().unwrap_or_default(),
            port: target.port.map(|port| port.to_string()).unwrap_or_default(),
            identity: target.identity.clone().unwrap_or_default(),
            folder: String::new(),
        }
    }

    /// What is wrong, field by field. Nothing is wrong with an empty optional
    /// field; the name and the host are required.
    pub fn problems(&self) -> Vec<Problem> {
        let mut found = Vec::new();
        let mut add = |field: Field, message: &str| {
            found.push(Problem {
                field,
                message: message.to_owned(),
            })
        };
        if self.name.trim().is_empty() {
            add(Field::Name, "Give it a name, for example Build server.");
        }
        let host = self.host.trim();
        if host.is_empty() {
            add(
                Field::Host,
                "Type the host: a name like build.example, an address like 192.168.1.20, or user@host.",
            );
        } else if host.starts_with('-') {
            add(Field::Host, "A host cannot start with a dash.");
        } else if host.contains(char::is_whitespace) {
            add(Field::Host, "A host has no spaces.");
        } else if host.contains('@') || host.starts_with("ssh://") {
            // Splitting happens as it is typed; what is left here is wrong.
            match address::parse_destination(host) {
                Ok(_) => {}
                Err(error) => add(Field::Host, &error.to_string()),
            }
        }
        let user = self.user.trim();
        if user.contains(char::is_whitespace) || user.contains('@') {
            add(Field::User, "A user name has no spaces or @.");
        }
        let port = self.port.trim();
        if !port.is_empty() && !port.parse::<u16>().is_ok_and(|port| port != 0) {
            add(Field::Port, "The port is a number from 1 to 65535.");
        }
        if self.identity.contains(['\n', '\r']) {
            add(Field::Identity, "Name one key file.");
        }
        let folder = self.folder.trim();
        if !folder.is_empty() && !address::is_absolute_path(folder) {
            add(
                Field::Folder,
                "Use a full path on that computer, such as /home/dev/code.",
            );
        }
        found
    }

    /// What will be saved: the user is left empty when none was typed, so
    /// `ssh` (or its configuration) decides.
    pub fn target(&self) -> Target {
        let optional = |text: &str| {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        };
        Target {
            host: self.host.trim().to_owned(),
            user: optional(&self.user),
            port: self.port.trim().parse().ok().filter(|port| *port != 0),
            identity: optional(&self.identity),
        }
    }

    /// The target as the commands on screen show it: the login name fills a
    /// missing user, so `ssh-copy-id` is complete.
    pub fn shown_target(&self, login: &str) -> Target {
        let mut target = self.target();
        if target.user.is_none() && !target.host.is_empty() {
            target.user = Some(login.to_owned());
        }
        target
    }

    /// The folder suggestions are read from: the start folder when given.
    pub fn start_folder(&self) -> Option<String> {
        let folder = self.folder.trim();
        (!folder.is_empty()).then(|| folder.to_owned())
    }
}

/// Host suggestions that start with what is typed, case ignored, the typed
/// text itself left out, at most `limit`.
pub fn matching_hosts<'a>(all: &'a [String], typed: &str, limit: usize) -> Vec<&'a str> {
    let typed = typed.trim().to_lowercase();
    all.iter()
        .filter(|host| host.to_lowercase().starts_with(&typed) && host.to_lowercase() != typed)
        .take(limit)
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(host: &str) -> Form {
        Form {
            name: "box".into(),
            host: host.into(),
            ..Form::default()
        }
    }

    #[test]
    fn the_host_field_splits_while_it_is_typed() {
        let split = |text: &str| split_host(text).map(|d| (d.user, d.host, d.port));
        assert_eq!(
            split("dev@box:2222"),
            Some((Some("dev".into()), "box".into(), Some(2222)))
        );
        assert_eq!(
            split("ssh://dev@box"),
            Some((Some("dev".into()), "box".into(), None))
        );
        assert_eq!(split("box:22"), Some((None, "box".into(), Some(22))));
        assert_eq!(
            split("dev@[::1]:2200"),
            Some((Some("dev".into()), "::1".into(), Some(2200)))
        );
        // Plain hosts, bare IPv6 and half-typed text are left alone.
        assert_eq!(split("box"), None);
        assert_eq!(split("fe80::1"), None);
        assert_eq!(split("dev@"), None);
        assert_eq!(split("box:"), None);
        assert_eq!(split("box:ssh"), None);
        assert_eq!(split(""), None);
    }

    #[test]
    fn a_form_without_problems_becomes_a_target() {
        let mut ok = form("box");
        ok.user = " dev ".into();
        ok.port = "2222".into();
        ok.identity = "/k/id".into();
        assert!(ok.problems().is_empty());
        let target = ok.target();
        assert_eq!(target.user.as_deref(), Some("dev"));
        assert_eq!(target.port, Some(2222));
        assert_eq!(target.identity.as_deref(), Some("/k/id"));
        assert_eq!(form("box").target().user, None);
    }

    #[test]
    fn each_field_has_its_own_specific_message() {
        let bad = Form {
            port: "70000".into(),
            user: "a b".into(),
            folder: "code".into(),
            ..Form::default()
        };
        let by_field: Vec<Field> = bad.problems().iter().map(|p| p.field).collect();
        assert_eq!(
            by_field,
            [
                Field::Name,
                Field::Host,
                Field::User,
                Field::Port,
                Field::Folder
            ]
        );
        let mut dash = form("-oProxyCommand=x");
        dash.name = "x".into();
        assert!(dash.problems()[0].message.contains("dash"));
        assert!(form("a b")
            .problems()
            .iter()
            .any(|p| p.message.contains("spaces")));
        let mut half = form("dev@");
        half.name = "x".into();
        assert_eq!(half.problems().len(), 1);
    }

    #[test]
    fn the_shown_target_uses_the_login_name_for_the_copy_line() {
        let shown = form("box").shown_target("alice");
        assert_eq!(shown.copy_id_line(), "ssh-copy-id alice@box");
        assert_eq!(
            form("box").target().user,
            None,
            "what is saved is untouched"
        );
        assert_eq!(
            form("").shown_target("alice").copy_id_line(),
            "ssh-copy-id user@host"
        );
    }

    #[test]
    fn the_howtos_follow_the_other_computers_platform() {
        let target = Target {
            host: "box".into(),
            user: Some("dev".into()),
            ..Target::default()
        };
        let text = |platform| {
            howtos(platform, &target, &[])[0]
                .lines
                .iter()
                .map(|l| l.text.clone() + l.command.as_deref().unwrap_or(""))
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert!(text(Platform::Mac).contains("Remote Login"));
        assert!(text(Platform::Linux).contains("openssh-server"));
        assert!(text(Platform::Linux).contains("systemctl enable --now ssh"));
        assert!(text(Platform::Windows).contains("not supported yet"));
    }

    #[test]
    fn the_key_row_says_which_keys_exist_and_never_their_contents() {
        let target = Target {
            host: "box".into(),
            user: Some("dev".into()),
            ..Target::default()
        };
        let none = howtos(Platform::Linux, &target, &[])[1].clone();
        assert!(none
            .lines
            .iter()
            .any(|l| l.command.as_deref() == Some("ssh-keygen -t ed25519")));
        let keys = [KeyFile {
            public: "id_ed25519.pub".into(),
            private: "/h/.ssh/id_ed25519".into(),
        }];
        let some = howtos(Platform::Linux, &target, &keys)[1].clone();
        assert!(some.lines[0].text.contains("id_ed25519.pub"));
        assert!(some
            .lines
            .iter()
            .any(|l| l.command.as_deref() == Some("ssh-copy-id dev@box")));
    }

    #[test]
    fn the_platform_choice_steps_and_stops_at_the_ends() {
        assert_eq!(Platform::Mac.step(false), Platform::Mac);
        assert_eq!(Platform::Mac.step(true), Platform::Linux);
        assert_eq!(Platform::Windows.step(true), Platform::Windows);
    }

    #[test]
    fn suggestions_match_by_prefix_without_repeating_the_typed_name() {
        let all = vec!["build".to_owned(), "Builder".to_owned(), "web".to_owned()];
        assert_eq!(matching_hosts(&all, "bu", 5), ["build", "Builder"]);
        assert_eq!(matching_hosts(&all, "build", 5), ["Builder"]);
        assert_eq!(matching_hosts(&all, "", 2), ["build", "Builder"]);
    }
}
