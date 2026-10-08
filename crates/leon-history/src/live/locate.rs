//! Where the transcript of a live session is on disk.
//!
//! The application knows an agent's own session id and the folder it was
//! started in; these functions turn that into the file to follow.
//!
//! * Claude Code keeps `<projects>/<project>/<session-id>.jsonl`, where
//!   `<project>` is the session's folder with every character that is not
//!   an ASCII letter or digit replaced by `-`. The sub-agents of a session
//!   write `<projects>/<project>/<session-id>/subagents/agent-<id>.jsonl`,
//!   each with an `agent-<id>.meta.json` beside it that names the tool call
//!   it was started by.
//! * Codex keeps `<sessions>/<year>/<month>/<day>/rollout-<time>-<id>.jsonl`.
//!
//! The path arithmetic is pure. The three functions that look at the disk
//! say so, and only list directories.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// How deep below Codex's sessions root rollouts are looked for.
const MAX_ROLLOUT_DEPTH: usize = 6;

/// The name of the folder Claude Code keeps the sessions of `cwd` in.
pub fn claude_project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Whether `id` can be a file name on its own: not empty, no separators,
/// not a way out of the folder.
fn is_plain_name(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0'])
}

/// Where Claude Code writes the transcript of session `session_id` started
/// in `cwd`, below its `projects` directory. `None` when the id cannot be a
/// file name.
///
/// This is where the file is expected, not a promise that it is there: a
/// session resumed from another folder stays in its first folder's project,
/// and very long folder names are shortened by Claude Code.
/// [`find_claude_transcript`] covers both.
pub fn claude_transcript_path(projects: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    is_plain_name(session_id).then(|| {
        projects
            .join(claude_project_dir_name(cwd))
            .join(format!("{session_id}.jsonl"))
    })
}

/// Finds the transcript of a Claude Code session on disk: the expected path
/// when it exists, else the file of that name in any project folder. Reads
/// directory entries only.
pub fn find_claude_transcript(projects: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    let expected = claude_transcript_path(projects, cwd, session_id)?;
    if expected.is_file() {
        return Some(expected);
    }
    let name = format!("{session_id}.jsonl");
    let mut found: Vec<PathBuf> = fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|project| project.path().join(&name))
        .filter(|candidate| candidate.is_file())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// The folder the sub-agents of the session at `transcript` write into.
pub fn claude_subagents_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join("subagents")
}

/// The transcript of one sub-agent of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentFile {
    /// The sub-agent's id: the `task` of [`crate::live::Beat::Detached`].
    pub agent: String,
    /// Its transcript, read with [`crate::live::Format::ClaudeSubagent`].
    pub path: PathBuf,
    /// Its description file, which may not exist (see [`SubagentMeta`]).
    pub meta: PathBuf,
}

/// Maps the file names found in a session's sub-agent folder to the
/// sub-agent transcripts among them, sorted by id. `transcript` is the
/// session's own file; `names` is the listing of
/// [`claude_subagents_dir`]`(transcript)`.
pub fn claude_subagent_files<S: AsRef<str>>(transcript: &Path, names: &[S]) -> Vec<SubagentFile> {
    let directory = claude_subagents_dir(transcript);
    let mut files: Vec<SubagentFile> = names
        .iter()
        .filter_map(|name| {
            let agent = name
                .as_ref()
                .strip_prefix("agent-")?
                .strip_suffix(".jsonl")?;
            is_plain_name(agent).then(|| SubagentFile {
                agent: agent.to_owned(),
                path: directory.join(name.as_ref()),
                meta: directory.join(format!("agent-{agent}.meta.json")),
            })
        })
        .collect();
    files.sort_by(|a, b| a.agent.cmp(&b.agent));
    files.dedup_by(|a, b| a.agent == b.agent);
    files
}

/// Where the transcript of sub-agent `agent` of the session at `transcript`
/// is written. `None` when the id cannot be a file name.
pub fn claude_subagent_path(transcript: &Path, agent: &str) -> Option<PathBuf> {
    is_plain_name(agent)
        .then(|| claude_subagents_dir(transcript).join(format!("agent-{agent}.jsonl")))
}

/// Lists the sub-agent transcripts of the session at `transcript` on disk.
/// A session without sub-agents has none. Reads directory entries only.
pub fn list_claude_subagents(transcript: &Path) -> Vec<SubagentFile> {
    let names: Vec<String> = fs::read_dir(claude_subagents_dir(transcript))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    claude_subagent_files(transcript, &names)
}

/// What Claude Code records about a sub-agent beside its transcript.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct SubagentMeta {
    /// The sub-agent's type, such as `Explore`.
    #[serde(rename = "agentType")]
    pub agent_type: Option<String>,
    /// The short description of its task.
    pub description: Option<String>,
    /// The id of the tool call that started it: the `id` of the
    /// [`crate::live::Beat::ToolStarted`] in the parent's transcript.
    #[serde(rename = "toolUseId")]
    pub tool_use_id: Option<String>,
}

impl SubagentMeta {
    /// Reads the bytes of an `agent-<id>.meta.json` file.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

/// Whether a file called `file_name` is the Codex rollout of `session_id`.
pub fn is_codex_rollout_of(file_name: &str, session_id: &str) -> bool {
    !session_id.is_empty()
        && file_name
            .strip_suffix(".jsonl")
            .is_some_and(|stem| stem.starts_with("rollout-") && stem.ends_with(session_id))
}

/// Finds the rollout of a Codex session below its `sessions` directory.
/// Walks the dated folders, newest names first; reads directory entries
/// only.
pub fn find_codex_rollout(sessions: &Path, session_id: &str) -> Option<PathBuf> {
    let mut pending = vec![(sessions.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = pending.pop() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&directory)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        let mut folders = Vec::new();
        for entry in entries {
            if entry.is_dir() {
                if depth < MAX_ROLLOUT_DEPTH {
                    folders.push((entry, depth + 1));
                }
            } else if entry
                .file_name()
                .is_some_and(|name| is_codex_rollout_of(&name.to_string_lossy(), session_id))
            {
                return Some(entry);
            }
        }
        // The stack pops the last pushed first: the newest date.
        pending.extend(folders);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn a_folder_becomes_its_project_name() {
        assert_eq!(claude_project_dir_name("/srv/api"), "-srv-api");
        assert_eq!(
            claude_project_dir_name("/home/u/.config/my_app v2"),
            "-home-u--config-my-app-v2"
        );
        assert_eq!(claude_project_dir_name("C:\\work\\api"), "C--work-api");
        assert_eq!(claude_project_dir_name("/srv/café"), "-srv-caf-");
    }

    #[test]
    fn the_transcript_is_expected_in_the_project_of_its_folder() {
        let projects = Path::new("/home/u/.claude/projects");
        assert_eq!(
            claude_transcript_path(projects, "/srv/api", "0198c0de-aaaa"),
            Some(projects.join("-srv-api").join("0198c0de-aaaa.jsonl"))
        );
    }

    #[test]
    fn an_id_that_is_not_a_file_name_has_no_path() {
        let projects = Path::new("/p");
        for id in ["", "..", "../../etc/passwd", "a/b", "a\\b"] {
            assert_eq!(claude_transcript_path(projects, "/srv/api", id), None);
            assert_eq!(claude_subagent_path(Path::new("/p/x/s.jsonl"), id), None);
            assert_eq!(find_claude_transcript(projects, "/srv/api", id), None);
        }
    }

    #[test]
    fn a_transcript_is_found_where_expected_or_in_another_project() {
        let root = tempfile::tempdir().unwrap();
        let expected = root.path().join("-srv-api/s1.jsonl");
        touch(&expected);
        touch(&root.path().join("-srv-other/s2.jsonl"));

        assert_eq!(
            find_claude_transcript(root.path(), "/srv/api", "s1"),
            Some(expected)
        );
        // Resumed from another folder: the file stays in its first project.
        assert_eq!(
            find_claude_transcript(root.path(), "/srv/api", "s2"),
            Some(root.path().join("-srv-other/s2.jsonl"))
        );
        assert_eq!(find_claude_transcript(root.path(), "/srv/api", "s3"), None);
        assert_eq!(
            find_claude_transcript(&root.path().join("missing"), "/srv/api", "s1"),
            None
        );
    }

    #[test]
    fn sub_agent_files_are_picked_from_a_listing() {
        let transcript = Path::new("/p/-srv-api/s1.jsonl");
        let directory = Path::new("/p/-srv-api/s1/subagents");
        assert_eq!(claude_subagents_dir(transcript), directory);

        let files = claude_subagent_files(
            transcript,
            &[
                "agent-b2.jsonl",
                "agent-b2.meta.json",
                "agent-a1.jsonl",
                "notes.txt",
                "agent-.jsonl",
                "other-a3.jsonl",
            ],
        );
        assert_eq!(
            files,
            [
                SubagentFile {
                    agent: "a1".into(),
                    path: directory.join("agent-a1.jsonl"),
                    meta: directory.join("agent-a1.meta.json"),
                },
                SubagentFile {
                    agent: "b2".into(),
                    path: directory.join("agent-b2.jsonl"),
                    meta: directory.join("agent-b2.meta.json"),
                },
            ]
        );
        assert_eq!(
            claude_subagent_path(transcript, "a1"),
            Some(directory.join("agent-a1.jsonl"))
        );
        assert_eq!(
            claude_subagent_files(transcript, &[] as &[&str]),
            Vec::new()
        );
    }

    #[test]
    fn sub_agents_are_listed_from_disk() {
        let root = tempfile::tempdir().unwrap();
        let transcript = root.path().join("-srv-api/s1.jsonl");
        touch(&transcript);
        assert_eq!(list_claude_subagents(&transcript), Vec::new());

        touch(&root.path().join("-srv-api/s1/subagents/agent-a1.jsonl"));
        touch(&root.path().join("-srv-api/s1/subagents/agent-a1.meta.json"));
        let listed = list_claude_subagents(&transcript);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].agent, "a1");
    }

    #[test]
    fn a_sub_agents_description_names_the_call_that_started_it() {
        let meta = SubagentMeta::parse(
            br#"{"agentType":"Explore","description":"Map the parser","toolUseId":"t9","spawnDepth":1}"#,
        )
        .unwrap();
        assert_eq!(meta.agent_type.as_deref(), Some("Explore"));
        assert_eq!(meta.description.as_deref(), Some("Map the parser"));
        assert_eq!(meta.tool_use_id.as_deref(), Some("t9"));
        assert_eq!(SubagentMeta::parse(b"{}"), Some(SubagentMeta::default()));
        assert_eq!(SubagentMeta::parse(b"not json"), None);
    }

    #[test]
    fn a_rollout_is_recognised_by_the_id_its_name_ends_with() {
        let id = "0198c0de-aaaa-7bbb-8ccc-0123456789ab";
        assert!(is_codex_rollout_of(
            &format!("rollout-2026-03-01T10-00-00-{id}.jsonl"),
            id
        ));
        assert!(!is_codex_rollout_of(&format!("other-{id}.jsonl"), id));
        assert!(!is_codex_rollout_of(&format!("rollout-x-{id}.txt"), id));
        assert!(!is_codex_rollout_of("rollout-x.jsonl", ""));
    }

    #[test]
    fn a_rollout_is_found_in_the_dated_folders() {
        let root = tempfile::tempdir().unwrap();
        let id = "0198c0de-aaaa-7bbb-8ccc-0123456789ab";
        let other = "0198c0de-bbbb-7bbb-8ccc-0123456789ab";
        let wanted = root
            .path()
            .join(format!("2026/03/01/rollout-2026-03-01T10-00-00-{id}.jsonl"));
        touch(&wanted);
        touch(&root.path().join(format!(
            "2025/12/31/rollout-2025-12-31T10-00-00-{other}.jsonl"
        )));
        assert_eq!(find_codex_rollout(root.path(), id), Some(wanted));
        assert_eq!(find_codex_rollout(root.path(), "no-such-session"), None);
        assert_eq!(find_codex_rollout(&root.path().join("missing"), id), None);
    }
}
