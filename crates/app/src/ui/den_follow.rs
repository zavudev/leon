//! Following the transcripts of the live sessions, for the Den.
//!
//! [`Followers`] holds one [`Follower`] per live session whose transcript is
//! on this computer, and one per sub-agent of those sessions that is alive.
//! [`Followers::poll`] is given the sessions that are wanted now and answers
//! with what each transcript grew by, as beats. It reads files and lists
//! folders, so it runs on the background executor, never on the window's
//! thread; nothing here knows the window.
//!
//! A session is followed from the start of its file the first time, which is
//! what makes its level (the tools it has used) right; after that only what
//! was appended is read. A session that is no longer wanted is forgotten,
//! and with no session wanted a poll touches no file at all.

use std::collections::HashMap;
use std::path::PathBuf;

use leon_core::AgentId;
use leon_history::live::{
    find_claude_transcript, find_codex_rollout, list_claude_subagents, Beat, Follower, Format,
    Start, SubagentMeta,
};
use leon_history::HistoryRoots;

/// A live session whose transcript is wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// The live session's id.
    pub live: u64,
    /// Its agent.
    pub agent: AgentId,
    /// The folder it runs in.
    pub cwd: String,
    /// The agent's own id of its session.
    pub session: String,
    /// The tool calls of its sub-agents that are alive, and the sub-agents'
    /// own ids where the transcript told them: only these are followed.
    pub subagents: Vec<(String, Option<String>)>,
}

/// What one sub-agent's transcript grew by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubReport {
    /// The id of the tool call that started it.
    pub tool: String,
    /// Its own id: the one its file is named after.
    pub agent: String,
    /// Its file was replaced: what was read before is void.
    pub restarted: bool,
    /// The new beats.
    pub beats: Vec<Beat>,
}

/// What one session's transcript grew by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The live session's id.
    pub live: u64,
    /// The agent's own id of the session that was read.
    pub session: String,
    /// The file was replaced or is read for the first time: the picture
    /// starts again from these beats.
    pub restarted: bool,
    /// The new beats.
    pub beats: Vec<Beat>,
    /// When the transcript was last written, in seconds since the Unix
    /// epoch, as its file says: how long a session with no terminal of
    /// ours has been quiet.
    pub written: Option<i64>,
    /// Its sub-agents.
    pub subs: Vec<SubReport>,
}

struct Tracked {
    session: String,
    path: PathBuf,
    follower: Follower,
    fresh: bool,
    /// Sub-agent id to the tool call its `meta.json` names, once read.
    metas: HashMap<String, Option<String>>,
    subs: HashMap<String, (String, Follower, bool)>,
}

/// The transcripts being followed.
#[derive(Default)]
pub struct Followers {
    roots: HistoryRoots,
    tracked: HashMap<u64, Tracked>,
}

impl Followers {
    /// Where the agents' files are looked for, when the settings move them.
    pub fn set_roots(&mut self, roots: HistoryRoots) {
        if self.roots != roots {
            self.roots = roots;
            self.tracked.clear();
        }
    }

    /// How many sessions are followed.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.tracked.len()
    }

    fn locate(&self, wanted: &Wanted) -> Option<(PathBuf, Format)> {
        let format = Format::of(wanted.agent)?;
        let path = match format {
            Format::Codex => {
                find_codex_rollout(self.roots.codex_sessions.as_deref()?, &wanted.session)
            }
            _ => find_claude_transcript(
                self.roots.claude_projects.as_deref()?,
                &wanted.cwd,
                &wanted.session,
            ),
        }?;
        Some((path, format))
    }

    /// Reads what the wanted sessions' transcripts grew by. A session whose
    /// file is not found yet is asked for again on the next poll; one that
    /// is no longer wanted is forgotten. Blocking file reads: call it off
    /// the window's thread.
    pub fn poll(&mut self, wanted: &[Wanted]) -> Vec<Report> {
        self.tracked
            .retain(|live, _| wanted.iter().any(|one| one.live == *live));
        let mut reports = Vec::new();
        for one in wanted {
            // The agent moved to another session in the same terminal.
            if self
                .tracked
                .get(&one.live)
                .is_some_and(|tracked| tracked.session != one.session)
            {
                self.tracked.remove(&one.live);
            }
            if !self.tracked.contains_key(&one.live) {
                let Some((path, format)) = self.locate(one) else {
                    continue;
                };
                self.tracked.insert(
                    one.live,
                    Tracked {
                        session: one.session.clone(),
                        follower: Follower::new(&path, format, Start::Beginning),
                        path,
                        fresh: true,
                        metas: HashMap::new(),
                        subs: HashMap::new(),
                    },
                );
            }
            let Some(tracked) = self.tracked.get_mut(&one.live) else {
                continue;
            };
            let Ok(polled) = tracked.follower.poll() else {
                // The file went away: look for it again next time.
                self.tracked.remove(&one.live);
                continue;
            };
            let restarted = polled.restarted || std::mem::take(&mut tracked.fresh);
            let subs = tracked.poll_subagents(&one.subagents);
            let written = std::fs::metadata(&tracked.path)
                .and_then(|file| file.modified())
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_secs() as i64);
            reports.push(Report {
                live: one.live,
                session: one.session.clone(),
                restarted,
                beats: polled.beats,
                written,
                subs,
            });
        }
        reports
    }
}

impl Tracked {
    /// Follows the transcripts of the sub-agents that are alive, and only
    /// those: a long session leaves many finished ones behind.
    fn poll_subagents(&mut self, alive: &[(String, Option<String>)]) -> Vec<SubReport> {
        self.subs
            .retain(|tool, _| alive.iter().any(|(wanted, _)| wanted == tool));
        if alive.is_empty() {
            return Vec::new();
        }
        let missing = alive.iter().any(|(tool, _)| !self.subs.contains_key(tool));
        if missing {
            for file in list_claude_subagents(&self.path) {
                // Which call started it: told by the transcript, or by the
                // description the agent writes beside the sub-agent's file.
                let known = alive
                    .iter()
                    .find(|(_, agent)| agent.as_deref() == Some(file.agent.as_str()))
                    .map(|(tool, _)| tool.clone());
                let tool = known.or_else(|| {
                    self.metas
                        .entry(file.agent.clone())
                        .or_insert_with(|| {
                            std::fs::read(&file.meta)
                                .ok()
                                .and_then(|bytes| SubagentMeta::parse(&bytes))
                                .and_then(|meta| meta.tool_use_id)
                        })
                        .clone()
                });
                let Some(tool) = tool.filter(|tool| alive.iter().any(|(wanted, _)| wanted == tool))
                else {
                    continue;
                };
                self.subs.entry(tool).or_insert_with(|| {
                    (
                        file.agent.clone(),
                        Follower::new(&file.path, Format::ClaudeSubagent, Start::Beginning),
                        true,
                    )
                });
            }
        }
        let mut reports = Vec::new();
        for (tool, (agent, follower, fresh)) in &mut self.subs {
            let Ok(polled) = follower.poll() else {
                continue;
            };
            reports.push(SubReport {
                tool: tool.clone(),
                agent: agent.clone(),
                restarted: polled.restarted || std::mem::take(fresh),
                beats: polled.beats,
            });
        }
        reports.sort_by(|a, b| a.tool.cmp(&b.tool));
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_history::live::{claude_project_dir_name, ToolKind};
    use std::io::Write;
    use std::path::Path;

    const SESSION: &str = "0a1b2c3d-0000-4000-8000-000000000001";

    fn assistant_tool(id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    fn tool_result(id: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"ok"}}]}}}}"#
        )
    }

    fn append(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        transcript: PathBuf,
        followers: Followers,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let transcript = projects
            .join(claude_project_dir_name("/work/leon"))
            .join(format!("{SESSION}.jsonl"));
        let mut followers = Followers::default();
        followers.set_roots(HistoryRoots {
            claude_projects: Some(projects),
            codex_sessions: None,
            opencode_db: None,
        });
        Fixture {
            _dir: dir,
            transcript,
            followers,
        }
    }

    fn wanted(subagents: &[(&str, Option<&str>)]) -> Wanted {
        Wanted {
            live: 1,
            agent: AgentId::CLAUDE,
            cwd: "/work/leon".to_owned(),
            session: SESSION.to_owned(),
            subagents: subagents
                .iter()
                .map(|(tool, agent)| ((*tool).to_owned(), agent.map(str::to_owned)))
                .collect(),
        }
    }

    fn tools(beats: &[Beat]) -> Vec<(String, ToolKind)> {
        beats
            .iter()
            .filter_map(|beat| match beat {
                Beat::ToolStarted { name, kind, .. } => Some((name.clone(), *kind)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_session_is_read_from_its_start_once_and_then_only_what_is_appended() {
        let mut f = fixture();
        // No file yet: asked for again later, nothing is followed.
        assert!(f.followers.poll(&[wanted(&[])]).is_empty());
        assert_eq!(f.followers.len(), 0);

        append(
            &f.transcript,
            &[
                assistant_tool("t1", "Read", r#"{"file_path":"/work/leon/a.rs"}"#),
                tool_result("t1"),
            ],
        );
        let first = f.followers.poll(&[wanted(&[])]);
        assert_eq!(first.len(), 1);
        assert!(first[0].restarted, "the first reading starts the picture");
        assert_eq!((first[0].live, first[0].session.as_str()), (1, SESSION));
        assert_eq!(
            tools(&first[0].beats),
            vec![("Read".to_owned(), ToolKind::Read)]
        );

        // Nothing appended: a report with no beat, and not a restart.
        let idle = f.followers.poll(&[wanted(&[])]);
        assert!(idle[0].beats.is_empty() && !idle[0].restarted);

        append(
            &f.transcript,
            &[assistant_tool("t2", "Bash", r#"{"command":"cargo test"}"#)],
        );
        let next = f.followers.poll(&[wanted(&[])]);
        assert_eq!(
            tools(&next[0].beats),
            vec![("Bash".to_owned(), ToolKind::Run)]
        );
        assert!(!next[0].restarted);
    }

    #[test]
    fn a_session_that_is_no_longer_wanted_is_forgotten_and_nothing_is_read_for_nobody() {
        let mut f = fixture();
        append(&f.transcript, &[tool_result("t0")]);
        f.followers.poll(&[wanted(&[])]);
        assert_eq!(f.followers.len(), 1);
        assert!(f.followers.poll(&[]).is_empty());
        assert_eq!(f.followers.len(), 0);
        // Wanted again: read from the start again.
        assert!(f.followers.poll(&[wanted(&[])])[0].restarted);
    }

    #[test]
    fn an_agent_leon_cannot_read_or_an_unknown_session_is_not_followed() {
        let mut f = fixture();
        append(&f.transcript, &[tool_result("t0")]);
        let mut other = wanted(&[]);
        other.agent = AgentId::OPENCODE;
        assert!(f.followers.poll(&[other]).is_empty());
        let mut unknown = wanted(&[]);
        unknown.session = "ffffffff-0000-4000-8000-000000000009".to_owned();
        assert!(f.followers.poll(&[unknown]).is_empty());
    }

    #[test]
    fn the_terminal_moving_to_another_session_starts_the_reading_again() {
        let mut f = fixture();
        append(&f.transcript, &[tool_result("t0")]);
        f.followers.poll(&[wanted(&[])]);
        let second = "0a1b2c3d-0000-4000-8000-000000000002";
        let other = f.transcript.with_file_name(format!("{second}.jsonl"));
        append(
            &other,
            &[assistant_tool("x1", "Grep", r#"{"pattern":"Overlay"}"#)],
        );
        let mut moved = wanted(&[]);
        moved.session = second.to_owned();
        let report = f.followers.poll(&[moved]);
        assert!(report[0].restarted);
        assert_eq!(report[0].session, second);
        assert_eq!(
            tools(&report[0].beats),
            vec![("Grep".to_owned(), ToolKind::Search)]
        );
    }

    #[test]
    fn only_the_sub_agents_that_are_alive_are_followed_found_by_their_description() {
        let mut f = fixture();
        append(&f.transcript, &[tool_result("t0")]);
        let subagents = f.transcript.with_extension("").join("subagents");
        // An old one that ended, and the one that is alive.
        for (agent, tool) in [("aaa111", "t-old"), ("bbb222", "t-live")] {
            append(
                &subagents.join(format!("agent-{agent}.jsonl")),
                &[assistant_tool(
                    "s1",
                    "Read",
                    r#"{"file_path":"/work/leon/x.rs"}"#,
                )],
            );
            std::fs::write(
                subagents.join(format!("agent-{agent}.meta.json")),
                format!(r#"{{"agentType":"Explore","toolUseId":"{tool}"}}"#),
            )
            .unwrap();
        }
        // Nobody alive: the folder is not even listed.
        assert!(f.followers.poll(&[wanted(&[])])[0].subs.is_empty());

        let report = f.followers.poll(&[wanted(&[("t-live", None)])]);
        let subs = &report[0].subs;
        assert_eq!(subs.len(), 1);
        assert_eq!(
            (subs[0].tool.as_str(), subs[0].agent.as_str()),
            ("t-live", "bbb222")
        );
        assert!(subs[0].restarted);
        assert_eq!(
            tools(&subs[0].beats),
            vec![("Read".to_owned(), ToolKind::Read)]
        );

        // It ended: its follower is dropped.
        assert!(f.followers.poll(&[wanted(&[])])[0].subs.is_empty());
        // One whose id the transcript told needs no description file.
        let report = f.followers.poll(&[wanted(&[("t-x", Some("aaa111"))])]);
        assert_eq!(report[0].subs[0].agent, "aaa111");
        assert_eq!(report[0].subs[0].tool, "t-x");
    }
}
