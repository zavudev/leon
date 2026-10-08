//! Several accounts of one agent.
//!
//! An [`Account`] is an agent, a name and a set of environment variables. The
//! variables are what makes the agent another account: typically the agent's own
//! configuration folder (`CLAUDE_CONFIG_DIR` for Claude Code, `CODEX_HOME` for
//! Codex; see [`crate::AgentSpec::config_env`]), so a session started with the
//! account signs in, keeps its history and reads its settings from that folder.
//! The agent's own setup, with no variable added, is always there too: it is the
//! implicit account [`DEFAULT_NAME`], never stored.
//!
//! Accounts are the user's data, stored in the settings like the custom agents.
//! The application hands them to a small registry ([`register`], one per thread:
//! the window's) so a name can be looked up from an id in the interface; a session remembers its account by the stable
//! id, never the name, so renaming one keeps every session where it was.
//!
//! Which account a new session uses is decided by [`resolve`], a pure function
//! of the agent, the accounts and the default-account setting.

use serde::{Deserialize, Serialize};

use crate::AgentId;

/// The name of the agent's own setup, which every agent has and which cannot be
/// given to an account.
pub const DEFAULT_NAME: &str = "default";

/// The longest name, variable name and value accepted.
const MAX_NAME: usize = 40;
const MAX_VALUE: usize = 1024;

/// The most variables one account sets.
const MAX_VARIABLES: usize = 16;

/// A named set of environment variables for one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Its stable id, the agent and a slug of the name; kept when it is
    /// renamed. Sessions remember this, not the name.
    pub id: String,
    /// The agent it belongs to.
    pub agent: AgentId,
    /// The name shown.
    pub name: String,
    /// The variables added to the environment of a terminal started with it, in
    /// order. Values may start with `~`, which stands for the home folder of
    /// the machine the session runs on.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// What is wrong with an account the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    /// There is no name.
    NoName,
    /// The name is too long or has a control character.
    BadName,
    /// The name is the agent's own setup, which is always there.
    ReservedName,
    /// Another account of the agent has that name.
    NameTaken,
    /// A variable has a name no shell accepts.
    BadVariable(String),
    /// A value has a control character or is too long.
    BadValue(String),
    /// The same variable is set twice.
    Repeated(String),
    /// There are more variables than an account takes.
    TooMany,
    /// A variable was typed without its `=`.
    NotAssignment(String),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoName => f.write_str("Give the account a name."),
            Self::BadName => f.write_str("The name is too long or has a control character."),
            Self::ReservedName => write!(
                f,
                "\"{DEFAULT_NAME}\" is the agent's own setup, which is always there: pick another name."
            ),
            Self::NameTaken => f.write_str("Another account of this agent has that name."),
            Self::BadVariable(name) => write!(
                f,
                "{name:?} is not a variable name: use letters, digits and _, not starting with a digit."
            ),
            Self::BadValue(name) => write!(
                f,
                "The value of {name} is too long or has a control character."
            ),
            Self::Repeated(name) => write!(f, "{name} is set twice."),
            Self::TooMany => write!(f, "An account sets at most {MAX_VARIABLES} variables."),
            Self::NotAssignment(word) => {
                write!(f, "{word:?} is not NAME=value.")
            }
        }
    }
}

impl std::error::Error for AccountError {}

/// Whether `name` can be the name of an environment variable.
fn valid_variable(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_NAME
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The variables of a line the user typed: `NAME=value` words split the way a
/// shell would (`CLAUDE_CONFIG_DIR='~/my folder'`). The empty line sets none.
pub fn parse_variables(text: &str) -> Result<Vec<(String, String)>, AccountError> {
    crate::agent::split_words(text)
        .into_iter()
        .map(|word| match word.split_once('=') {
            Some((name, value)) => Ok((name.to_owned(), value.to_owned())),
            None => Err(AccountError::NotAssignment(word)),
        })
        .collect()
}

/// The variables of an account as one line, the way [`parse_variables`] reads
/// them back.
pub fn variables_line(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(name, value)| {
            let plain = !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.:/@%+,~".contains(c));
            if plain {
                format!("{name}={value}")
            } else {
                format!("{name}='{}'", value.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The slug of a name: lower-case letters and digits, `-` between them.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(24).collect()
}

impl Account {
    /// A new account as typed, with an id that no account in `existing` has.
    pub fn new(
        agent: AgentId,
        name: &str,
        env: Vec<(String, String)>,
        existing: &[Account],
    ) -> Result<Self, AccountError> {
        let slug = slug(name);
        let slug = if slug.is_empty() {
            "account".to_owned()
        } else {
            slug
        };
        let base = format!("{}-{slug}", agent.as_str());
        let mut id = base.clone();
        let mut n = 2;
        while existing.iter().any(|account| account.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        let account = Self {
            id,
            agent,
            name: name.trim().to_owned(),
            env,
        };
        account.check(existing)?;
        Ok(account)
    }

    /// Checks the account against the others: `others` are the other accounts
    /// of the settings (this one, when it is among them, is skipped).
    pub fn check(&self, others: &[Account]) -> Result<(), AccountError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(AccountError::NoName);
        }
        if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
            return Err(AccountError::BadName);
        }
        if name.eq_ignore_ascii_case(DEFAULT_NAME) {
            return Err(AccountError::ReservedName);
        }
        if others.iter().any(|other| {
            other.id != self.id
                && other.agent == self.agent
                && other.name.trim().eq_ignore_ascii_case(name)
        }) {
            return Err(AccountError::NameTaken);
        }
        if self.env.len() > MAX_VARIABLES {
            return Err(AccountError::TooMany);
        }
        for (index, (variable, value)) in self.env.iter().enumerate() {
            if !valid_variable(variable) {
                return Err(AccountError::BadVariable(variable.clone()));
            }
            if value.chars().count() > MAX_VALUE || value.chars().any(char::is_control) {
                return Err(AccountError::BadValue(variable.clone()));
            }
            if self.env[..index]
                .iter()
                .any(|(earlier, _)| earlier == variable)
            {
                return Err(AccountError::Repeated(variable.clone()));
            }
        }
        Ok(())
    }

    /// The value this account gives to the variable `name`, when it sets it.
    pub fn value_of(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.as_str())
    }

    /// The folder this account gives to its agent's configuration, when the
    /// agent has a verified variable for it and the account sets it.
    pub fn config_dir(&self) -> Option<&str> {
        let variable = self.agent.spec()?.config_env.as_deref()?;
        self.value_of(variable).filter(|value| !value.is_empty())
    }
}

/// A value that may start with `~` (the home folder) as a path: `home` is the
/// home folder of the machine the value is for. `None` when the value starts
/// with `~` and the home folder is not known, or names another user's (`~bob`),
/// which Leon does not resolve; a value that does not start with `~` is returned
/// as it is.
pub fn expand_home(value: &str, home: Option<&str>) -> Option<String> {
    let Some(rest) = value.strip_prefix('~') else {
        return Some(value.to_owned());
    };
    let home = home.filter(|home| !home.is_empty())?;
    match rest {
        "" => Some(home.to_owned()),
        _ if rest.starts_with(['/', '\\']) => Some(format!(
            "{}/{}",
            home.trim_end_matches(['/', '\\']),
            &rest[1..]
        )),
        _ => None,
    }
}

/// The accounts the settings hold (one JSON object each). An entry that is not
/// understood, is not valid, or repeats an id or a name is left out.
pub fn from_entries(entries: &[String]) -> Vec<Account> {
    let mut accounts: Vec<Account> = Vec::new();
    for entry in entries {
        let Ok(account) = serde_json::from_str::<Account>(entry) else {
            continue;
        };
        if accounts.iter().any(|known| known.id == account.id) {
            continue;
        }
        if account.check(&accounts).is_ok() {
            accounts.push(account);
        }
    }
    accounts
}

/// The accounts of `agent`, in the order the settings hold them.
pub fn of_agent(accounts: &[Account], agent: AgentId) -> Vec<&Account> {
    accounts
        .iter()
        .filter(|account| account.agent == agent)
        .collect()
}

/// The account with this id.
pub fn by_id<'a>(accounts: &'a [Account], id: &str) -> Option<&'a Account> {
    accounts.iter().find(|account| account.id == id)
}

/// What a new session of an agent does about accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick<'a> {
    /// The agent's own setup: nothing is added to its environment.
    Plain,
    /// This account, chosen by the default-account setting.
    Account(&'a Account),
    /// The person chooses: the agent's own setup or one of these accounts.
    Ask(Vec<&'a Account>),
}

/// The name the default-account setting gives `agent`. Each entry is
/// `agent=name` (`claude=work`); `name` may be [`DEFAULT_NAME`] for the agent's
/// own setup. The first entry of the agent counts.
pub fn default_for(agent: AgentId, defaults: &[String]) -> Option<&str> {
    defaults.iter().find_map(|entry| {
        let (tag, name) = entry.split_once('=')?;
        (tag.trim().eq_ignore_ascii_case(agent.as_str())).then(|| name.trim())
    })
}

/// Which account a new session of `agent` starts with.
///
/// An agent without accounts starts as it always did. With accounts, the agent's
/// own setup is one more choice, so the person is asked, unless the
/// default-account setting names the one to use. A default that names an account
/// that no longer exists asks too: the session is not started with another
/// account's sign-in by guesswork.
pub fn resolve<'a>(agent: AgentId, accounts: &'a [Account], defaults: &[String]) -> Pick<'a> {
    let named = of_agent(accounts, agent);
    if named.is_empty() {
        return Pick::Plain;
    }
    match default_for(agent, defaults) {
        Some(name) if name.eq_ignore_ascii_case(DEFAULT_NAME) => Pick::Plain,
        Some(name) => named
            .iter()
            .find(|account| account.name.trim().eq_ignore_ascii_case(name))
            .map_or(Pick::Ask(named.clone()), |account| Pick::Account(account)),
        None => Pick::Ask(named),
    }
}

thread_local! {
    /// The accounts the application knows. Per thread, like the interface size:
    /// the window reads names on its own thread, and the settings that fill it
    /// are the window's, so two windows (or two tests) never see each other's.
    static ACCOUNTS: std::cell::RefCell<Vec<Account>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Makes `accounts` the ones [`all`] and [`find`] know on this thread, in place
/// of the ones before. The application calls it with the accounts of the
/// settings.
pub fn register(accounts: Vec<Account>) {
    ACCOUNTS.with(|list| *list.borrow_mut() = accounts);
}

/// Every account the application knows.
pub fn all() -> Vec<Account> {
    ACCOUNTS.with(|list| list.borrow().clone())
}

/// The account with this id, when it is known.
pub fn find(id: &str) -> Option<Account> {
    ACCOUNTS.with(|list| {
        list.borrow()
            .iter()
            .find(|account| account.id == id)
            .cloned()
    })
}

/// The name to show for the account with this id: its name, or the id for an
/// account that was removed.
pub fn name_of(id: &str) -> String {
    find(id).map_or_else(|| id.to_owned(), |account| account.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(agent: AgentId, name: &str, env: &[(&str, &str)]) -> Account {
        Account::new(
            agent,
            name,
            env.iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
            &[],
        )
        .unwrap()
    }

    #[test]
    fn an_account_gets_an_id_from_its_agent_and_name_and_never_repeats_one() {
        let first = account(AgentId::CLAUDE, "Work (Acme)", &[]);
        assert_eq!(first.id, "claude-work-acme");
        let second = Account::new(
            AgentId::CLAUDE,
            "Work Acme!",
            Vec::new(),
            std::slice::from_ref(&first),
        );
        // The slug repeats, the name differs only in punctuation: still two names.
        assert_eq!(second.unwrap().id, "claude-work-acme-2");
        let codex = Account::new(AgentId::CODEX, "Work (Acme)", Vec::new(), &[first]).unwrap();
        assert_eq!(codex.id, "codex-work-acme");
    }

    #[test]
    fn names_are_checked_per_agent() {
        let work = account(AgentId::CLAUDE, "Work", &[]);
        let existing = [work];
        assert_eq!(
            Account::new(AgentId::CLAUDE, " work ", Vec::new(), &existing),
            Err(AccountError::NameTaken)
        );
        assert!(Account::new(AgentId::CODEX, "work", Vec::new(), &existing).is_ok());
        assert_eq!(
            Account::new(AgentId::CLAUDE, "   ", Vec::new(), &existing),
            Err(AccountError::NoName)
        );
        assert_eq!(
            Account::new(AgentId::CLAUDE, "Default", Vec::new(), &existing),
            Err(AccountError::ReservedName)
        );
        assert_eq!(
            Account::new(AgentId::CLAUDE, "a\nb", Vec::new(), &existing),
            Err(AccountError::BadName)
        );
        assert_eq!(
            Account::new(AgentId::CLAUDE, &"x".repeat(41), Vec::new(), &existing),
            Err(AccountError::BadName)
        );
    }

    #[test]
    fn variables_are_checked_before_they_can_reach_a_shell() {
        let try_env = |name: &str, value: &str| {
            Account::new(
                AgentId::CLAUDE,
                "x",
                vec![(name.to_owned(), value.to_owned())],
                &[],
            )
        };
        assert!(try_env("CLAUDE_CONFIG_DIR", "~/.claude-work").is_ok());
        assert!(try_env("A_1", "").is_ok());
        for bad in ["", "1A", "A B", "A=B", "A-B", "A;rm", "A$(x)", "é"] {
            assert_eq!(
                try_env(bad, "v"),
                Err(AccountError::BadVariable(bad.to_owned())),
                "{bad:?}"
            );
        }
        assert_eq!(
            try_env("A", "line\nbreak"),
            Err(AccountError::BadValue("A".into()))
        );
        assert_eq!(
            try_env("A", &"v".repeat(1025)),
            Err(AccountError::BadValue("A".into()))
        );
        let twice = Account::new(
            AgentId::CLAUDE,
            "x",
            vec![("A".into(), "1".into()), ("A".into(), "2".into())],
            &[],
        );
        assert_eq!(twice, Err(AccountError::Repeated("A".into())));
        let many: Vec<(String, String)> =
            (0..17).map(|n| (format!("V{n}"), String::new())).collect();
        assert_eq!(
            Account::new(AgentId::CLAUDE, "x", many, &[]),
            Err(AccountError::TooMany)
        );
    }

    #[test]
    fn typed_variables_are_split_like_a_shell_and_print_back_the_same() {
        let parsed = parse_variables("CLAUDE_CONFIG_DIR='~/my folder' B=2").unwrap();
        assert_eq!(
            parsed,
            [
                ("CLAUDE_CONFIG_DIR".to_owned(), "~/my folder".to_owned()),
                ("B".to_owned(), "2".to_owned())
            ]
        );
        assert_eq!(
            variables_line(&parsed),
            "CLAUDE_CONFIG_DIR='~/my folder' B=2"
        );
        assert_eq!(parse_variables("  ").unwrap(), Vec::new());
        assert_eq!(
            parse_variables("oops"),
            Err(AccountError::NotAssignment("oops".into()))
        );
        let quoted = vec![("Q".to_owned(), "it's".to_owned())];
        assert_eq!(parse_variables(&variables_line(&quoted)).unwrap(), quoted);
        let empty = vec![("E".to_owned(), String::new())];
        assert_eq!(parse_variables(&variables_line(&empty)).unwrap(), empty);
    }

    #[test]
    fn the_configuration_folder_is_the_variable_the_agent_table_names() {
        let claude = account(
            AgentId::CLAUDE,
            "work",
            &[("CLAUDE_CONFIG_DIR", "/home/me/.claude-work"), ("X", "1")],
        );
        assert_eq!(claude.config_dir(), Some("/home/me/.claude-work"));
        let codex = account(AgentId::CODEX, "work", &[("CODEX_HOME", "/h/.codex-work")]);
        assert_eq!(codex.config_dir(), Some("/h/.codex-work"));
        // The wrong agent's variable is not the folder of this agent.
        let mixed = account(AgentId::CODEX, "x", &[("CLAUDE_CONFIG_DIR", "/h/.c")]);
        assert_eq!(mixed.config_dir(), None);
        // An agent whose folder variable was not verified has no folder.
        let other = account(AgentId::OPENCODE, "x", &[("XDG_DATA_HOME", "/h/d")]);
        assert_eq!(other.config_dir(), None);
        assert_eq!(account(AgentId::CLAUDE, "bare", &[]).config_dir(), None);
    }

    #[test]
    fn a_home_folder_is_expanded_only_where_it_is_known() {
        assert_eq!(expand_home("/abs/x", None), Some("/abs/x".to_owned()));
        assert_eq!(expand_home("rel/x", None), Some("rel/x".to_owned()));
        assert_eq!(expand_home("~", Some("/h")), Some("/h".to_owned()));
        assert_eq!(
            expand_home("~/.claude-work", Some("/h")),
            Some("/h/.claude-work".to_owned())
        );
        assert_eq!(
            expand_home("~/x", Some("/h/")),
            Some("/h/x".to_owned()),
            "no doubled separator"
        );
        assert_eq!(expand_home("~/x", None), None);
        assert_eq!(expand_home("~/x", Some("")), None);
        assert_eq!(expand_home("~bob/x", Some("/h")), None);
        assert_eq!(expand_home("a~b", Some("/h")), Some("a~b".to_owned()));
    }

    #[test]
    fn the_settings_entries_are_read_and_the_bad_ones_left_out() {
        let work = account(AgentId::CLAUDE, "work", &[("CLAUDE_CONFIG_DIR", "/w")]);
        let json = |account: &Account| serde_json::to_string(account).unwrap();
        let mut renamed_twin = work.clone();
        renamed_twin.name = "other".into();
        let mut same_name = work.clone();
        same_name.id = "claude-work-9".into();
        let entries = vec![
            json(&work),
            "not json".to_owned(),
            json(&renamed_twin),
            json(&same_name),
            r#"{"id":"x","agent":"Not An Id","name":"n"}"#.to_owned(),
            json(&account(AgentId::CODEX, "work", &[])),
        ];
        let read = from_entries(&entries);
        assert_eq!(
            read.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["claude-work", "codex-work"]
        );
        // An entry written without variables reads as an account with none.
        let bare = r#"{"id":"claude-b","agent":"claude","name":"b"}"#;
        assert_eq!(from_entries(&[bare.to_owned()])[0].env, Vec::new());
    }

    fn resolve_for<'a>(agent: AgentId, accounts: &'a [Account], defaults: &[&str]) -> Pick<'a> {
        let defaults: Vec<String> = defaults.iter().map(|d| (*d).to_owned()).collect();
        resolve(agent, accounts, &defaults)
    }

    #[test]
    fn an_agent_without_accounts_starts_as_it_always_did() {
        let accounts = [account(AgentId::CODEX, "work", &[])];
        assert_eq!(resolve_for(AgentId::CLAUDE, &accounts, &[]), Pick::Plain);
        assert_eq!(resolve_for(AgentId::CLAUDE, &[], &[]), Pick::Plain);
        // A default for an agent that has no accounts changes nothing.
        assert_eq!(
            resolve_for(AgentId::CLAUDE, &accounts, &["claude=work"]),
            Pick::Plain
        );
    }

    #[test]
    fn one_named_account_makes_two_choices_and_asks() {
        let work = account(AgentId::CLAUDE, "work", &[]);
        let accounts = [work.clone()];
        assert_eq!(
            resolve_for(AgentId::CLAUDE, &accounts, &[]),
            Pick::Ask(vec![&work])
        );
    }

    #[test]
    fn the_default_account_setting_skips_the_question() {
        let work = account(AgentId::CLAUDE, "Work", &[]);
        let home = account(AgentId::CLAUDE, "Home", &[]);
        let accounts = [work.clone(), home.clone()];
        assert_eq!(
            resolve_for(AgentId::CLAUDE, &accounts, &["claude=work"]),
            Pick::Account(&work)
        );
        // Case and spaces do not matter, and the first entry of the agent wins.
        assert_eq!(
            resolve_for(
                AgentId::CLAUDE,
                &accounts,
                &[" Claude = HOME ", "claude=work"]
            ),
            Pick::Account(&home)
        );
        // The agent's own setup can be the default.
        assert_eq!(
            resolve_for(AgentId::CLAUDE, &accounts, &["claude=default"]),
            Pick::Plain
        );
        // Another agent's entry is not this agent's.
        assert_eq!(
            resolve_for(AgentId::CLAUDE, &accounts, &["codex=work"]),
            Pick::Ask(vec![&work, &home])
        );
    }

    #[test]
    fn a_default_that_names_a_removed_account_asks_instead_of_guessing() {
        let work = account(AgentId::CLAUDE, "work", &[]);
        let accounts = [work.clone()];
        assert_eq!(
            resolve_for(AgentId::CLAUDE, &accounts, &["claude=gone"]),
            Pick::Ask(vec![&work])
        );
        assert_eq!(default_for(AgentId::CLAUDE, &["claude".to_owned()]), None);
    }

    #[test]
    fn the_registry_knows_names_by_id_and_keeps_the_id_of_a_removed_account() {
        // The registry is this thread's own: no other test sees it.
        let work = account(AgentId::CLAUDE, "Work", &[]);
        register(vec![work.clone()]);
        assert_eq!(find("claude-work"), Some(work));
        assert_eq!(name_of("claude-work"), "Work");
        register(Vec::new());
        assert_eq!(find("claude-work"), None);
        assert_eq!(name_of("claude-work"), "claude-work");
        assert!(all().is_empty());
    }
}
