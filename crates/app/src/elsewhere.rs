//! Sessions that run in another terminal: tying the agent processes of a
//! machine to Leon's history sessions.
//!
//! `leon_remote::processes` says what runs and what each process's arguments
//! and state file claim; [`resolve`] maps that to the stored sessions and
//! says how sure it is:
//!
//! * **Certain**: the process names the session (its arguments, or Claude
//!   Code's own `~/.claude/sessions/<pid>.json`).
//! * **Likely**: nothing names it; the process is the only agent of its kind
//!   in its folder and the session is the latest of that folder
//!   (`--continue`), or was written after the process started. When two
//!   processes compete for a folder, or another process already holds the
//!   session for certain, nothing is guessed.
//!
//! Everything here is pure: the engine runs the scan and calls [`resolve`];
//! the UI calls [`foreign`] to leave out Leon's own terminals.

use chrono::{Local, TimeZone};
use leon_core::{AgentId, Session, SessionId};
use leon_remote::processes::{AgentProcess, AppBundle, Resumes, Scan};
use std::collections::HashSet;

/// How sure a match is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// The process names the session.
    Certain,
    /// The best fit among the sessions of its folder.
    Likely,
}

/// What produced a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Claude Code's own state file for the pid.
    StateFile,
    /// The session id in the process's arguments.
    Arguments,
    /// `--continue` or `--last`: the latest session of the folder.
    Continue,
    /// The session of the folder written after the process started.
    RecentInFolder,
}

impl Signal {
    /// In words, for the diagnostic.
    pub fn label(self) -> &'static str {
        match self {
            Self::StateFile => "claude state file",
            Self::Arguments => "arguments",
            Self::Continue => "--continue/--last: latest session of the folder",
            Self::RecentInFolder => "latest session of the folder written after it started",
        }
    }
}

/// A process matched to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// How sure.
    pub confidence: Confidence,
    /// What said so.
    pub signal: Signal,
}

/// An agent process, and the session it holds when that is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its pid.
    pub pid: u32,
    /// Its agent.
    pub agent: AgentId,
    /// Its terminal device.
    pub tty: Option<String>,
    /// When it started, in unix seconds.
    pub started: Option<i64>,
    /// Where it runs.
    pub cwd: Option<String>,
    /// The agent's own id of the session, when the scan learned one (also
    /// when the history does not know it yet).
    pub external_id: Option<String>,
    /// The stored session it holds.
    pub session: Option<SessionId>,
    /// How that was concluded.
    pub link: Option<Link>,
    /// Whether it runs below this Leon process (always false over SSH).
    pub leon_child: bool,
    /// The processes above it, nearest first: how a terminal of Leon's own
    /// recognises the agent that runs in it.
    pub ancestors: Vec<u32>,
    /// The terminal application it runs in, when the process tree says.
    pub app: Option<AppBundle>,
}

impl Found {
    /// Whether the match is certain.
    pub fn is_certain(&self) -> bool {
        matches!(
            self.link,
            Some(Link {
                confidence: Confidence::Certain,
                ..
            })
        )
    }

    /// "pid 70645, started 20:50, ttys000, iTerm": what a person can use to
    /// find the process.
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("pid {}", self.pid)];
        if let Some(at) = self
            .started
            .and_then(|s| Local.timestamp_opt(s, 0).single())
        {
            parts.push(format!("started {}", at.format("%H:%M")));
        }
        parts.extend(self.tty.clone());
        parts.extend(self.app.as_ref().map(|app| format!("in {}", app.name)));
        parts.join(", ")
    }
}

fn same_folder(a: &str, b: &str) -> bool {
    leon_core::path::key(a) == leon_core::path::key(b)
}

/// Maps the agent processes of `scan` to `sessions` (those of the machine
/// that was scanned). `leon_pid` is this Leon's pid when the machine is this
/// computer: what runs below it is Leon's own.
pub fn resolve(scan: &Scan, sessions: &[Session], leon_pid: Option<u32>) -> Vec<Found> {
    let by_id = |agent: AgentId, id: &str| {
        sessions
            .iter()
            .find(|session| session.agent == agent && session.external_id == id)
    };
    let mut found: Vec<Found> = scan
        .agents
        .iter()
        .map(|process| Found {
            pid: process.pid,
            agent: process.agent,
            tty: process.tty.clone(),
            started: process.started,
            cwd: process.cwd.clone(),
            external_id: None,
            session: None,
            link: None,
            leon_child: leon_pid.is_some_and(|root| scan.descends_from(process.pid, root)),
            ancestors: scan.ancestors(process.pid),
            app: scan.owning_app(process.pid),
        })
        .collect();

    // Certain: the process says which session it holds.
    let mut claimed: HashSet<(AgentId, String)> = HashSet::new();
    for (entry, process) in found.iter_mut().zip(&scan.agents) {
        let named = match (&process.state_session, &process.resumes) {
            (Some(id), _) => Some((id.clone(), Signal::StateFile)),
            (None, Resumes::Id(id)) => Some((id.clone(), Signal::Arguments)),
            _ => None,
        };
        if let Some((id, signal)) = named {
            claimed.insert((process.agent, id.clone()));
            entry.session = by_id(process.agent, &id).map(|session| session.id.clone());
            entry.external_id = Some(id);
            entry.link = Some(Link {
                confidence: Confidence::Certain,
                signal,
            });
        }
    }

    // Likely: the only unnamed process of its kind in a folder.
    let unnamed: Vec<usize> = (0..found.len())
        .filter(|&at| found[at].link.is_none() && found[at].cwd.is_some())
        .collect();
    for &at in &unnamed {
        let (agent, cwd) = (found[at].agent, found[at].cwd.clone().unwrap_or_default());
        let rivals = scan
            .agents
            .iter()
            .zip(&found)
            .filter(|(other, entry)| {
                other.agent == agent
                    && entry.link.is_none()
                    && entry.cwd.as_deref().is_some_and(|c| same_folder(c, &cwd))
            })
            .count();
        if rivals != 1 {
            continue;
        }
        let process: &AgentProcess = &scan.agents[at];
        let candidates = sessions.iter().filter(|session| {
            session.agent == agent
                && same_folder(&session.cwd, &cwd)
                && !claimed.contains(&(agent, session.external_id.clone()))
        });
        let chosen = match &process.resumes {
            Resumes::Latest => candidates
                .max_by_key(|session| session.updated_at)
                .map(|session| (session, Signal::Continue)),
            Resumes::Unnamed => process.started.and_then(|started| {
                candidates
                    .filter(|session| session.updated_at.timestamp() >= started)
                    .max_by_key(|session| session.updated_at)
                    .map(|session| (session, Signal::RecentInFolder))
            }),
            Resumes::Id(_) => None,
        };
        if let Some((session, signal)) = chosen {
            found[at].session = Some(session.id.clone());
            found[at].external_id = Some(session.external_id.clone());
            found[at].link = Some(Link {
                confidence: Confidence::Likely,
                signal,
            });
        }
    }
    found
}

/// One of Leon's own terminals on a machine: an agent started in Leon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnTerminal {
    /// Its agent.
    pub agent: AgentId,
    /// The folder it runs in.
    pub cwd: String,
    /// The history session it resumed, when it did.
    pub session: Option<SessionId>,
}

/// What runs in another terminal: on this computer, everything that is not
/// below this Leon (ancestry is exact); on another machine, where Leon cannot
/// see its own processes, everything except what Leon's own terminals there
/// account for, one process per terminal, by session or else by agent and
/// folder.
pub fn foreign(found: &[Found], local: bool, own: &[OwnTerminal]) -> Vec<Found> {
    if local {
        return found.iter().filter(|f| !f.leon_child).cloned().collect();
    }
    let mut left: Vec<&Found> = found.iter().collect();
    for terminal in own {
        let at = left
            .iter()
            .position(|f| {
                f.agent == terminal.agent
                    && match (&terminal.session, &f.session) {
                        (Some(mine), Some(theirs)) => mine == theirs,
                        _ => f
                            .cwd
                            .as_deref()
                            .is_some_and(|cwd| same_folder(cwd, &terminal.cwd)),
                    }
            })
            .or_else(|| {
                left.iter().position(|f| {
                    f.agent == terminal.agent && f.session.is_none() && f.cwd.is_none()
                })
            });
        if let Some(at) = at {
            left.remove(at);
        }
    }
    left.into_iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use leon_core::MachineId;
    use leon_remote::processes::parse_scan;

    const NOW: i64 = 1_000_000;

    fn session(agent: AgentId, external: &str, cwd: &str, updated: i64) -> Session {
        Session {
            id: SessionId::from_string(format!("s-{external}")),
            agent,
            external_id: external.to_owned(),
            machine_id: MachineId::local(),
            cwd: cwd.to_owned(),
            project_id: None,
            title: format!("title {external}"),
            model: None,
            started_at: Utc.timestamp_opt(updated - 60, 0).unwrap(),
            updated_at: Utc.timestamp_opt(updated, 0).unwrap(),
            message_count: 3,
            sort_order: None,
        }
    }

    const A: &str = "0a1b2c3d-1111-4222-8333-444455556666";
    const B: &str = "0a1b2c3d-7777-4888-9999-aaaabbbbcccc";

    fn scan(lines: &[&str]) -> Scan {
        parse_scan(&format!("now={NOW}\n{}\n", lines.join("\n")))
    }

    fn only(found: Vec<Found>) -> Found {
        assert_eq!(found.len(), 1, "{found:?}");
        found.into_iter().next().unwrap()
    }

    #[test]
    fn an_id_in_the_arguments_is_a_certain_match() {
        let found = only(resolve(
            &scan(&[&format!("A 10 1 ttys0 00:30 claude --resume {A}")]),
            &[session(AgentId::CLAUDE, A, "/w", NOW - 500)],
            None,
        ));
        assert_eq!(
            found.session,
            Some(SessionId::from_string(format!("s-{A}")))
        );
        assert_eq!(
            found.link,
            Some(Link {
                confidence: Confidence::Certain,
                signal: Signal::Arguments
            })
        );
        assert!(found.is_certain());
    }

    #[test]
    fn claudes_state_file_beats_the_arguments() {
        // Resumed one session, then `/resume`d another inside.
        let found = only(resolve(
            &scan(&[
                &format!("A 10 1 ttys0 00:30 claude --resume {A}"),
                &format!(
                    "S {{\"pid\":10,\"sessionId\":\"{B}\",\"startedAt\":{}}}",
                    (NOW - 30) * 1000
                ),
            ]),
            &[
                session(AgentId::CLAUDE, A, "/w", NOW - 500),
                session(AgentId::CLAUDE, B, "/w", NOW - 400),
            ],
            None,
        ));
        assert_eq!(found.external_id.as_deref(), Some(B));
        assert_eq!(found.link.unwrap().signal, Signal::StateFile);
    }

    #[test]
    fn an_id_the_history_does_not_know_is_kept_without_a_session() {
        let found = only(resolve(
            &scan(&[&format!("A 10 1 ttys0 00:30 claude --resume {A}")]),
            &[],
            None,
        ));
        assert_eq!(found.session, None);
        assert_eq!(found.external_id.as_deref(), Some(A));
        assert!(found.is_certain());
    }

    #[test]
    fn continue_is_the_latest_session_of_the_folder_and_only_likely() {
        let found = only(resolve(
            &scan(&["A 10 1 ttys0 00:30 claude --continue", "C 10 /w"]),
            &[
                session(AgentId::CLAUDE, A, "/w", NOW - 900),
                session(AgentId::CLAUDE, B, "/w", NOW - 800),
                session(AgentId::CLAUDE, "other", "/elsewhere", NOW - 10),
                session(AgentId::CODEX, "codex-one", "/w", NOW - 5),
            ],
            None,
        ));
        assert_eq!(found.external_id.as_deref(), Some(B));
        assert_eq!(
            found.link,
            Some(Link {
                confidence: Confidence::Likely,
                signal: Signal::Continue
            })
        );
        assert!(!found.is_certain());
    }

    #[test]
    fn a_fresh_process_matches_the_session_written_after_it_started() {
        // Started 30 s ago: the session touched 10 s ago is its; the one from
        // yesterday is not.
        let sessions = [
            session(AgentId::CODEX, "old", "/w", NOW - 86_400),
            session(AgentId::CODEX, "new", "/w", NOW - 10),
        ];
        let found = only(resolve(
            &scan(&["A 10 1 ttys0 00:30 codex", "C 10 /w/"]),
            &sessions,
            None,
        ));
        assert_eq!(found.external_id.as_deref(), Some("new"));
        assert_eq!(found.link.unwrap().signal, Signal::RecentInFolder);
        // With nothing newer than its start, nothing is guessed.
        let found = only(resolve(
            &scan(&["A 10 1 ttys0 00:30 codex", "C 10 /w"]),
            &sessions[..1],
            None,
        ));
        assert_eq!(found.link, None);
        assert_eq!(found.session, None);
    }

    #[test]
    fn two_unnamed_processes_in_one_folder_are_not_guessed() {
        let found = resolve(
            &scan(&[
                "A 10 1 ttys0 00:30 codex",
                "A 11 1 ttys1 00:20 codex",
                "C 10 /w",
                "C 11 /w",
            ]),
            &[session(AgentId::CODEX, "new", "/w", NOW - 5)],
            None,
        );
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|f| f.link.is_none()));
    }

    #[test]
    fn a_session_another_process_holds_for_certain_is_not_guessed_for_a_second() {
        let found = resolve(
            &scan(&[
                &format!("A 10 1 ttys0 00:30 claude --resume {A}"),
                "A 11 1 ttys1 00:20 claude --continue",
                "C 10 /w",
                "C 11 /w",
            ]),
            &[session(AgentId::CLAUDE, A, "/w", NOW - 5)],
            None,
        );
        let second = found.iter().find(|f| f.pid == 11).unwrap();
        assert_eq!(second.link, None);
    }

    #[test]
    fn what_runs_below_this_leon_is_its_own_and_the_rest_is_elsewhere() {
        let scan = scan(&[
            "T 1 0 /sbin/launchd",
            "T 100 1 leon",
            "T 101 100 /bin/zsh",
            "T 102 101 claude",
            "T 200 1 -zsh",
            "T 201 200 claude",
            "T 300 1 leon",
            "T 301 300 claude",
            &format!("A 102 101 ttys1 00:30 claude --resume {A}"),
            &format!("A 201 200 ttys2 00:30 claude --resume {B}"),
            "A 301 300 ttys3 00:30 claude",
        ]);
        let found = resolve(&scan, &[], Some(100));
        let own: Vec<(u32, bool)> = found.iter().map(|f| (f.pid, f.leon_child)).collect();
        assert_eq!(own, [(102, true), (201, false), (301, false)]);
        // A second Leon's session is elsewhere for this one.
        let away: Vec<u32> = foreign(&found, true, &[]).iter().map(|f| f.pid).collect();
        assert_eq!(away, [201, 301]);
    }

    #[test]
    fn over_ssh_leons_own_terminals_account_for_their_processes() {
        let sessions = [
            session(AgentId::CLAUDE, A, "/srv/a", NOW - 100),
            session(AgentId::CLAUDE, B, "/srv/b", NOW - 100),
        ];
        let found = resolve(
            &scan(&[
                &format!("A 10 1 pts/0 00:30 claude --resume {A}"),
                &format!("A 11 1 pts/1 00:30 claude --resume {B}"),
            ]),
            &sessions,
            None,
        );
        let own = [OwnTerminal {
            agent: AgentId::CLAUDE,
            cwd: "/srv/a".into(),
            session: Some(SessionId::from_string(format!("s-{A}"))),
        }];
        let away = foreign(&found, false, &own);
        assert_eq!(away.len(), 1);
        assert_eq!(away[0].pid, 11);
        // Without terminals of its own, both are elsewhere.
        assert_eq!(foreign(&found, false, &[]).len(), 2);
    }

    #[test]
    fn a_found_process_describes_itself_for_the_notice() {
        let found = only(resolve(
            &scan(&[&format!("A 70645 2 ttys000 00:30 claude --resume {A}")]),
            &[],
            None,
        ));
        let text = found.describe();
        assert!(text.starts_with("pid 70645, started "), "{text}");
        assert!(text.ends_with(", ttys000"), "{text}");
    }
}
