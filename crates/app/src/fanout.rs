//! One prompt, several agents: what will be made, decided without a window.
//!
//! The person types a prompt, ticks two or more agents and picks a base, and
//! Leon makes one worktree per agent and starts each agent in its own with the
//! prompt already on its launch line, so the sessions sit side by side in the
//! tree and can be compared. [`plan`] is the pure function from that request
//! to the plan: for each agent the branch (a slug of the prompt and the agent's
//! id), the folder (the setting `worktree_location`, the same function the
//! engine uses to make any worktree), and the exact line that will be typed
//! into its terminal ([`crate::launch::command_line_prompted`], which quotes
//! the prompt). An agent that cannot be given the prompt is not in the plan
//! but in the list of those left out, with the reason, so one agent's problem
//! never stops the others and each is reported by name.
//!
//! The window ([`crate::ui`]) makes the worktrees through the engine, one
//! after the other, and starts the sessions the plan describes.

use crate::address;
use crate::launch::{self, Flavor, LaunchPrefs};
use leon_core::AgentId;

/// The fewest agents worth a comparison.
pub const LEAST_AGENTS: usize = 2;

/// The longest slug of the prompt in a branch name, in characters.
const SLUG_MAX: usize = 28;

/// What the person asked for, and what the plan needs to know of the project.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    /// The prompt, as [`launch::clean_prompt`] returns it.
    pub prompt: &'a str,
    /// The agents ticked, in the order they were.
    pub agents: &'a [AgentId],
    /// The project's folder on its machine.
    pub root: &'a str,
    /// The setting `worktree_location`.
    pub location: &'a str,
    /// The branches the project's worktrees already have.
    pub taken: &'a [String],
    /// How the shell of the machine quotes.
    pub flavor: Flavor,
    /// What the settings change about how agents start.
    pub prefs: &'a LaunchPrefs,
}

/// One worktree and the agent that starts in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The agent.
    pub agent: AgentId,
    /// The branch of the new worktree.
    pub branch: String,
    /// Where the worktree goes.
    pub path: String,
    /// What is typed into its terminal (without the Enter).
    pub line: String,
}

/// An agent the prompt cannot be given to, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    /// The agent.
    pub agent: AgentId,
    /// The reason, a sentence.
    pub why: String,
}

/// What [`plan`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The worktrees to make, in the order the agents were ticked.
    pub items: Vec<Item>,
    /// The agents that were left out.
    pub left: Vec<Left>,
}

/// The branch names a new one must not take: those of the project's worktrees
/// and every ref git lists (`refs`, as [`crate::engine::Engine::base_refs`]
/// answers: local branches, then remote-tracking ones such as `origin/main`),
/// so a prompt given again is numbered instead of refused by git.
pub fn taken_branches(worktrees: impl IntoIterator<Item = String>, refs: &[String]) -> Vec<String> {
    let mut taken: Vec<String> = worktrees.into_iter().collect();
    for name in refs {
        if !taken.contains(name) {
            taken.push(name.clone());
        }
    }
    taken
}

/// The plan for `request`, or why there is none: the prompt cannot be typed
/// into a terminal, or fewer than [`LEAST_AGENTS`] different agents were
/// ticked.
pub fn plan(request: &Request) -> Result<Plan, String> {
    let prompt = launch::clean_prompt(request.prompt)?;
    let mut agents: Vec<AgentId> = Vec::new();
    for agent in request.agents {
        if !agents.contains(agent) {
            agents.push(*agent);
        }
    }
    if agents.len() < LEAST_AGENTS {
        return Err("Tick two agents or more.".to_owned());
    }
    let slug = prompt_slug(&prompt);
    let mut used: Vec<String> = request.taken.to_vec();
    let mut plan = Plan {
        items: Vec::new(),
        left: Vec::new(),
    };
    for agent in agents {
        match item(request, &prompt, &slug, agent, &used) {
            Ok(item) => {
                used.push(item.branch.clone());
                plan.items.push(item);
            }
            Err(why) => plan.left.push(Left { agent, why }),
        }
    }
    Ok(plan)
}

fn item(
    request: &Request,
    prompt: &str,
    slug: &str,
    agent: AgentId,
    used: &[String],
) -> Result<Item, String> {
    let spec = agent
        .spec()
        .ok_or_else(|| launch::LaunchError::UnknownAgent(agent).to_string())?;
    let line =
        launch::command_line_prompted(spec, prompt, request.flavor, request.prefs.agent(agent))
            .map_err(|error| error.to_string())?;
    let branch = free_branch(slug, agent, used);
    address::validate_branch(&branch).map_err(str::to_owned)?;
    let path = address::worktree_location(request.location, request.root, &branch)?;
    Ok(Item {
        agent,
        branch,
        path,
        line,
    })
}

/// The branch for `agent`: the slug and the agent's id, with a number before
/// the id when `used` has that name already.
fn free_branch(slug: &str, agent: AgentId, used: &[String]) -> String {
    let mut branch = format!("{slug}-{agent}");
    let mut number = 2;
    while used.contains(&branch) {
        branch = format!("{slug}-{number}-{agent}");
        number += 1;
    }
    branch
}

/// The first words of the prompt as part of a branch name: lower-case letters
/// and digits joined by `-`, at most [`SLUG_MAX`] characters and never a word
/// cut in two (the first word is, when it alone is longer). A prompt without a
/// letter or a digit in it is `prompt`.
pub fn prompt_slug(prompt: &str) -> String {
    let mut slug = String::new();
    let words = prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty());
    for word in words {
        let word = word.to_ascii_lowercase();
        let needed = if slug.is_empty() { 0 } else { 1 } + word.len();
        if slug.len() + needed > SLUG_MAX {
            if slug.is_empty() {
                slug = word.chars().take(SLUG_MAX).collect();
            }
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    if slug.is_empty() {
        "prompt".to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::LaunchError;

    const GEMINI: &str = "gemini";

    fn gemini() -> AgentId {
        AgentId::parse(GEMINI).unwrap()
    }

    fn aider() -> AgentId {
        AgentId::parse("aider").unwrap()
    }

    fn request<'a>(
        prompt: &'a str,
        agents: &'a [AgentId],
        taken: &'a [String],
        prefs: &'a LaunchPrefs,
    ) -> Request<'a> {
        Request {
            prompt,
            agents,
            root: "/srv/api",
            location: "{root}-worktrees/{branch}",
            taken,
            flavor: Flavor::Posix,
            prefs,
        }
    }

    #[test]
    fn each_agent_gets_a_branch_a_folder_and_a_launch_line() {
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE];
        let plan = plan(&request("Fix the login bug", &agents, &[], &prefs)).unwrap();
        assert_eq!(plan.left, []);
        let got: Vec<(&str, &str, &str)> = plan
            .items
            .iter()
            .map(|item| (item.branch.as_str(), item.path.as_str(), item.line.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "fix-the-login-bug-claude",
                    "/srv/api-worktrees/fix-the-login-bug-claude",
                    "claude 'Fix the login bug'"
                ),
                (
                    "fix-the-login-bug-codex",
                    "/srv/api-worktrees/fix-the-login-bug-codex",
                    "codex 'Fix the login bug'"
                ),
                (
                    "fix-the-login-bug-opencode",
                    "/srv/api-worktrees/fix-the-login-bug-opencode",
                    "opencode --prompt 'Fix the login bug'"
                ),
            ]
        );
    }

    #[test]
    fn the_folder_follows_the_setting() {
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, AgentId::CODEX];
        let mut asked = request("Fix it", &agents, &[], &prefs);
        asked.location = "{root}/.worktrees/{branch}";
        let plan = plan(&asked).unwrap();
        assert_eq!(plan.items[0].path, "/srv/api/.worktrees/fix-it-claude");
        asked.location = "{root}-worktrees";
        let plan = super::plan(&asked).unwrap();
        assert_eq!(plan.items, []);
        assert_eq!(plan.left.len(), 2, "a location that names no branch");
        assert!(plan.left[0].why.contains("{branch}"), "{:?}", plan.left);
    }

    #[test]
    fn a_branch_that_exists_gets_a_number_before_the_agent() {
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, AgentId::CODEX];
        let taken = ["fix-it-claude".to_owned(), "fix-it-2-claude".to_owned()];
        let plan = plan(&request("Fix it", &agents, &taken, &prefs)).unwrap();
        assert_eq!(plan.items[0].branch, "fix-it-3-claude");
        assert_eq!(plan.items[1].branch, "fix-it-codex");
    }

    #[test]
    fn a_branch_git_has_without_a_worktree_is_taken_too() {
        // The worktrees were removed; the branches stayed, as they do.
        let refs = ["main".to_owned(), "fix-it-claude".to_owned()];
        let taken = taken_branches(["fix-it-codex".to_owned(), "main".to_owned()], &refs);
        assert_eq!(taken, ["fix-it-codex", "main", "fix-it-claude"]);
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, AgentId::CODEX];
        let plan = plan(&request("Fix it", &agents, &taken, &prefs)).unwrap();
        assert_eq!(plan.items[0].branch, "fix-it-2-claude", "a rerun recovers");
        assert_eq!(plan.items[1].branch, "fix-it-2-codex");
    }

    #[test]
    fn an_agent_that_cannot_take_the_prompt_is_left_out_by_name_and_the_others_go_on() {
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, aider(), gemini()];
        let plan = plan(&request("Fix it", &agents, &[], &prefs)).unwrap();
        let branches: Vec<&str> = plan.items.iter().map(|item| item.branch.as_str()).collect();
        assert_eq!(branches, ["fix-it-claude", "fix-it-gemini"]);
        assert_eq!(
            plan.left,
            [Left {
                agent: aider(),
                why: LaunchError::NoPromptForm { agent: aider() }.to_string()
            }]
        );
    }

    #[test]
    fn a_shell_that_is_not_posix_leaves_every_agent_out_with_the_reason() {
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, AgentId::CODEX];
        let mut asked = request("Fix it", &agents, &[], &prefs);
        asked.flavor = Flavor::PowerShell;
        let plan = plan(&asked).unwrap();
        assert_eq!(plan.items, []);
        assert!(plan
            .left
            .iter()
            .all(|left| left.why.contains("POSIX shell")));
    }

    #[test]
    fn fewer_than_two_agents_or_a_prompt_that_cannot_be_typed_is_no_plan() {
        let prefs = LaunchPrefs::default();
        let one = [AgentId::CLAUDE];
        assert_eq!(
            plan(&request("Fix it", &one, &[], &prefs)).unwrap_err(),
            "Tick two agents or more."
        );
        let twice = [AgentId::CLAUDE, AgentId::CLAUDE];
        assert_eq!(
            plan(&request("Fix it", &twice, &[], &prefs)).unwrap_err(),
            "Tick two agents or more.",
            "the same agent twice is one agent"
        );
        let two = [AgentId::CLAUDE, AgentId::CODEX];
        for prompt in ["", "--help", "a\u{1b}b"] {
            assert!(
                plan(&request(prompt, &two, &[], &prefs)).is_err(),
                "{prompt:?}"
            );
        }
    }

    #[test]
    fn the_prompt_is_quoted_the_same_for_every_agent_and_the_settings_args_stay() {
        let mut prefs = LaunchPrefs::default();
        prefs.agents.insert(
            AgentId::CODEX,
            launch::AgentPrefs {
                args: vec!["--model".into(), "o3".into()],
                ..launch::AgentPrefs::default()
            },
        );
        let agents = [AgentId::CLAUDE, AgentId::CODEX];
        let plan = plan(&request("it's $HOME!", &agents, &[], &prefs)).unwrap();
        assert_eq!(plan.items[0].line, r"claude 'it'\''s $HOME!'");
        assert_eq!(plan.items[1].line, r"codex --model o3 'it'\''s $HOME!'");
    }

    #[test]
    fn a_prompt_of_several_lines_is_one_line_in_every_launch_line() {
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, gemini()];
        let plan = plan(&request("Refactor!\n\nThen test it.", &agents, &[], &prefs)).unwrap();
        for item in &plan.items {
            assert!(!item.line.contains('\n'), "{}", item.line);
            assert!(!item.line.contains('!'), "{}", item.line);
        }
        assert_eq!(plan.items[0].branch, "refactor-then-test-it-claude");
    }

    #[test]
    fn the_slug_keeps_whole_words_within_its_length() {
        assert_eq!(prompt_slug("Fix the login bug"), "fix-the-login-bug");
        assert_eq!(
            prompt_slug("Refactor the authentication module to use sessions"),
            "refactor-the-authentication"
        );
        assert_eq!(
            prompt_slug(&"x".repeat(60)),
            "x".repeat(SLUG_MAX),
            "a first word that is too long is cut"
        );
        assert_eq!(prompt_slug("  *** !!! "), "prompt");
        assert_eq!(prompt_slug("日本語 のみ"), "prompt");
        assert_eq!(prompt_slug("Add OAuth2/PKCE (v2)"), "add-oauth2-pkce-v2");
    }

    #[test]
    fn every_branch_of_a_plan_is_one_git_accepts() {
        let prefs = LaunchPrefs::default();
        let agents = [AgentId::CLAUDE, AgentId::CODEX, gemini()];
        for prompt in [
            "Fix it",
            "日本語",
            "!!!",
            "a/b..c",
            format!("{} y", "x".repeat(200)).as_str(),
        ] {
            let plan = plan(&request(prompt, &agents, &[], &prefs)).unwrap();
            assert_eq!(plan.items.len(), 3, "{prompt:?}");
            for item in plan.items {
                assert_eq!(address::validate_branch(&item.branch), Ok(()), "{item:?}");
            }
        }
    }
}
