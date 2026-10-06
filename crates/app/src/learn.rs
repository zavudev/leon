//! Learning the agent's own session id for a session started fresh in Leon.
//!
//! A new agent session has no id when it is launched, and Leon does not read
//! the screen to find one. After the launch it matches the session that
//! appears for that agent in that folder, with the strongest signal there is:
//!
//! 1. **The process** ([`by_process`]): the process scan names the session a
//!    live agent holds (Claude Code's own `~/.claude/sessions/<pid>.json`, or
//!    the id in its arguments) and the terminal's shell is an ancestor of that
//!    agent. Certain.
//! 2. **The folder** ([`by_folder`]) for Codex, opencode and the rest: the
//!    sessions of that folder created after the terminal started. With one
//!    terminal of that agent in the folder, its newest one; with several, a
//!    session is only linked when exactly one of them could own it, because
//!    guessing would put two terminals on one session. Likely, not certain.
//!
//! A session is never linked to two terminals. Both functions are pure; the
//! window feeds them and applies what they return.

use crate::elsewhere::{Found, Signal};
use chrono::{DateTime, Duration, Utc};
use leon_core::{AgentId, Session, SessionId};
use std::collections::{HashMap, HashSet};

/// How early a session may start before its terminal and still be its.
const SLACK_SECONDS: i64 = 2;

/// A terminal started in Leon that has no session id yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fresh {
    /// The live session's id.
    pub id: u64,
    /// Its agent.
    pub agent: AgentId,
    /// Its folder.
    pub cwd: String,
    /// When it started.
    pub started: DateTime<Utc>,
    /// The pid of the shell in the terminal, when it is known.
    pub shell_pid: Option<u32>,
}

/// What was learned for one terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learned {
    /// The live session's id.
    pub id: u64,
    /// The agent's own session id.
    pub external: String,
    /// The history row, when the importer already stored it.
    pub history: Option<SessionId>,
    /// How sure: `state-file`, `arguments` or `newest-in-folder`.
    pub how: &'static str,
}

/// The session ids that cannot be given again: those other terminals hold.
pub type Taken = HashSet<(AgentId, String)>;

/// Matches terminals to the agents running below their shells.
pub fn by_process(fresh: &[Fresh], found: &[Found], taken: &Taken) -> Vec<Learned> {
    let mut claimed = taken.clone();
    let mut learned = Vec::new();
    for terminal in fresh {
        let Some(shell) = terminal.shell_pid else {
            continue;
        };
        let hit = found.iter().find(|f| {
            f.agent == terminal.agent
                && f.ancestors.contains(&shell)
                && f.external_id
                    .as_ref()
                    .is_some_and(|id| !claimed.contains(&(f.agent, id.clone())))
                && matches!(
                    f.link.as_ref().map(|link| link.signal),
                    Some(Signal::StateFile | Signal::Arguments)
                )
        });
        if let Some(found) = hit {
            let external = found.external_id.clone().expect("checked above");
            claimed.insert((found.agent, external.clone()));
            learned.push(Learned {
                id: terminal.id,
                external,
                history: found.session.clone(),
                how: match found.link.as_ref().map(|link| link.signal) {
                    Some(Signal::Arguments) => "arguments",
                    _ => "state-file",
                },
            });
        }
    }
    learned
}

/// Matches terminals to the sessions of their folder created after they
/// started. `sessions` are the stored sessions of the machine.
pub fn by_folder(fresh: &[Fresh], sessions: &[Session], taken: &Taken) -> Vec<Learned> {
    let mut claimed = taken.clone();
    let mut groups: HashMap<(AgentId, String), Vec<&Fresh>> = HashMap::new();
    for terminal in fresh {
        groups
            .entry((terminal.agent, leon_core::path::key(&terminal.cwd)))
            .or_default()
            .push(terminal);
    }
    let mut learned = Vec::new();
    let mut keys: Vec<_> = groups.keys().cloned().collect();
    keys.sort_by(|a, b| (a.0.as_str(), &a.1).cmp(&(b.0.as_str(), &b.1)));
    for key in keys {
        let mut terminals = groups.remove(&key).unwrap_or_default();
        terminals.sort_by_key(|t| (t.started, t.id));
        let earliest = terminals[0].started - Duration::seconds(SLACK_SECONDS);
        let mut candidates: Vec<&Session> = sessions
            .iter()
            .filter(|s| {
                s.agent == key.0
                    && leon_core::path::key(&s.cwd) == key.1
                    && s.started_at >= earliest
                    && !claimed.contains(&(s.agent, s.external_id.clone()))
            })
            .collect();
        candidates.sort_by_key(|s| s.started_at);
        let link = |terminal: &Fresh, session: &Session, learned: &mut Vec<Learned>| {
            learned.push(Learned {
                id: terminal.id,
                external: session.external_id.clone(),
                history: Some(session.id.clone()),
                how: "newest-in-folder",
            });
        };
        if terminals.len() == 1 {
            // The newest session after the start is the one on screen.
            if let Some(newest) = candidates.last() {
                claimed.insert((newest.agent, newest.external_id.clone()));
                link(terminals[0], newest, &mut learned);
            }
            continue;
        }
        let mut free: Vec<&Fresh> = terminals;
        for session in candidates {
            let eligible: Vec<usize> = free
                .iter()
                .enumerate()
                .filter(|(_, t)| t.started - Duration::seconds(SLACK_SECONDS) <= session.started_at)
                .map(|(at, _)| at)
                .collect();
            // Two terminals could own it: leave it unlinked rather than guess.
            if let [only] = eligible[..] {
                let terminal = free.remove(only);
                claimed.insert((session.agent, session.external_id.clone()));
                link(terminal, session, &mut learned);
            }
        }
    }
    learned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elsewhere::{Confidence, Link};
    use chrono::TimeZone;
    use leon_core::MachineId;

    fn at(minute: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 10, minute, second)
            .unwrap()
    }

    fn fresh(id: u64, agent: AgentId, cwd: &str, started: DateTime<Utc>) -> Fresh {
        Fresh {
            id,
            agent,
            cwd: cwd.to_owned(),
            started,
            shell_pid: Some(1000 + id as u32),
        }
    }

    fn session(agent: AgentId, external: &str, cwd: &str, started: DateTime<Utc>) -> Session {
        Session {
            id: SessionId::from_string(format!("row-{external}")),
            agent,
            external_id: external.to_owned(),
            machine_id: MachineId::local(),
            cwd: cwd.to_owned(),
            project_id: None,
            title: String::new(),
            model: None,
            started_at: started,
            updated_at: started,
            message_count: 1,
        }
    }

    fn process(agent: AgentId, pid: u32, parent: u32, external: &str, signal: Signal) -> Found {
        Found {
            pid,
            agent,
            tty: None,
            started: None,
            cwd: None,
            external_id: Some(external.to_owned()),
            session: None,
            link: Some(Link {
                confidence: Confidence::Certain,
                signal,
            }),
            leon_child: true,
            ancestors: vec![parent, 1],
            app: None,
        }
    }

    #[test]
    fn the_state_file_ties_an_agent_to_the_terminal_whose_shell_is_above_it() {
        let terminals = [
            fresh(1, AgentId::CLAUDE, "/srv/api", at(0, 0)),
            fresh(2, AgentId::CLAUDE, "/srv/api", at(1, 0)),
        ];
        let found = [
            process(AgentId::CLAUDE, 5002, 1002, "sid-b", Signal::StateFile),
            process(AgentId::CLAUDE, 5001, 1001, "sid-a", Signal::StateFile),
        ];
        let learned = by_process(&terminals, &found, &Taken::new());
        assert_eq!(learned.len(), 2);
        assert_eq!((learned[0].id, learned[0].external.as_str()), (1, "sid-a"));
        assert_eq!((learned[1].id, learned[1].external.as_str()), (2, "sid-b"));
        assert!(learned.iter().all(|l| l.how == "state-file"));
    }

    #[test]
    fn a_process_that_is_not_below_the_shell_or_holds_a_taken_id_links_nothing() {
        let terminals = [fresh(1, AgentId::CLAUDE, "/srv/api", at(0, 0))];
        let elsewhere = process(AgentId::CLAUDE, 9, 4242, "sid", Signal::StateFile);
        assert!(by_process(&terminals, &[elsewhere], &Taken::new()).is_empty());
        let mine = process(AgentId::CLAUDE, 5001, 1001, "sid", Signal::StateFile);
        let taken: Taken = [(AgentId::CLAUDE, "sid".to_owned())].into();
        assert!(by_process(&terminals, &[mine], &taken).is_empty());
        // A guess from "the latest session" is not a signal either.
        let guessed = process(AgentId::CLAUDE, 5001, 1001, "sid", Signal::RecentInFolder);
        assert!(by_process(&terminals, &[guessed], &Taken::new()).is_empty());
    }

    #[test]
    fn one_terminal_takes_the_newest_session_of_its_folder_created_after_it_started() {
        let terminals = [fresh(1, AgentId::CODEX, "/srv/api", at(10, 0))];
        let sessions = [
            session(AgentId::CODEX, "before", "/srv/api", at(5, 0)),
            session(AgentId::CODEX, "first", "/srv/api", at(11, 0)),
            session(AgentId::CODEX, "newest", "/srv/api", at(12, 0)),
            session(AgentId::CODEX, "elsewhere", "/srv/web", at(13, 0)),
            session(AgentId::OPENCODE, "other-agent", "/srv/api", at(13, 0)),
        ];
        let learned = by_folder(&terminals, &sessions, &Taken::new());
        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0].external, "newest");
        assert_eq!(learned[0].how, "newest-in-folder");
        assert_eq!(
            learned[0].history,
            Some(SessionId::from_string("row-newest"))
        );
    }

    #[test]
    fn two_terminals_in_one_folder_get_their_own_sessions_in_order() {
        let terminals = [
            fresh(1, AgentId::OPENCODE, "/srv/api", at(0, 0)),
            fresh(2, AgentId::OPENCODE, "/srv/api", at(5, 0)),
        ];
        let sessions = [
            session(AgentId::OPENCODE, "s1", "/srv/api", at(1, 0)),
            session(AgentId::OPENCODE, "s2", "/srv/api", at(6, 0)),
        ];
        let learned = by_folder(&terminals, &sessions, &Taken::new());
        let pairs: Vec<_> = learned
            .iter()
            .map(|l| (l.id, l.external.as_str()))
            .collect();
        assert_eq!(pairs, [(1, "s1"), (2, "s2")]);
    }

    #[test]
    fn with_two_terminals_a_session_that_either_could_own_is_not_guessed() {
        let terminals = [
            fresh(1, AgentId::CODEX, "/srv/api", at(0, 0)),
            fresh(2, AgentId::CODEX, "/srv/api", at(5, 0)),
        ];
        // Created after both started: it could be either.
        let sessions = [session(AgentId::CODEX, "late", "/srv/api", at(6, 0))];
        assert!(by_folder(&terminals, &sessions, &Taken::new()).is_empty());
    }

    #[test]
    fn a_session_is_never_given_to_two_terminals() {
        let terminals = [
            fresh(1, AgentId::CODEX, "/srv/api", at(0, 0)),
            fresh(2, AgentId::CODEX, "/srv/web", at(0, 0)),
        ];
        let sessions = [session(AgentId::CODEX, "x", "/srv/api", at(1, 0))];
        // The session of /srv/api cannot go to the terminal in /srv/web.
        let learned = by_folder(&terminals, &sessions, &Taken::new());
        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0].id, 1);
        // Held by another terminal already: nothing is left to give.
        let taken: Taken = [(AgentId::CODEX, "x".to_owned())].into();
        assert!(by_folder(&terminals, &sessions, &taken).is_empty());
    }
}
