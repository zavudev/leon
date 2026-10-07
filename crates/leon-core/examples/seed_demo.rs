//! Seeds a store with the demo projects, worktrees and agent history used by
//! the screenshots in the README. Not part of the product: run it against a
//! throwaway `--data-dir` that points at the demo home.
//!
//! ```text
//! cargo run -p leon-core --example seed_demo -- <data-dir> <demo-home>
//! ```

use chrono::{Duration, Utc};
use leon_core::{MachineId, NewMessage, NewSession, NewWorktree, Role, Store};

fn main() {
    let mut args = std::env::args().skip(1);
    let data_dir = args
        .next()
        .expect("usage: seed_demo <data-dir> <demo-home>");
    let home = args
        .next()
        .expect("usage: seed_demo <data-dir> <demo-home>");
    std::fs::create_dir_all(&data_dir).expect("data dir");

    let store = Store::open(std::path::Path::new(&data_dir).join("leon.db")).expect("open store");
    let local = MachineId::local();

    // ------------------------------------------------------------ projects --
    let atlas = store
        .add_project(&local, "atlas", &format!("{home}/code/atlas"))
        .expect("atlas");
    let ledger = store
        .add_project(&local, "ledger", &format!("{home}/code/ledger"))
        .expect("ledger");

    let atlas_worktrees = store
        .replace_worktrees(
            &atlas.id,
            vec![
                worktree(&format!("{home}/code/atlas"), "main", true),
                worktree(
                    &format!("{home}/code/atlas-rate-limits"),
                    "rate-limits",
                    false,
                ),
                worktree(
                    &format!("{home}/code/atlas-login-timeout"),
                    "login-timeout",
                    false,
                ),
            ],
        )
        .expect("atlas worktrees");
    store
        .replace_worktrees(
            &ledger.id,
            vec![
                worktree(&format!("{home}/code/ledger"), "main", true),
                worktree(
                    &format!("{home}/code/ledger-csv-export"),
                    "csv-export",
                    false,
                ),
            ],
        )
        .expect("ledger worktrees");

    // The rate-limits branch is merged into main, so its worktree is done.
    if let Some(done) = atlas_worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some("rate-limits"))
    {
        store.set_merged(&done.id, Some(true)).expect("merged");
    }

    // ------------------------------------------------------------- history --
    let now = Utc::now();
    let seed = |external: &str,
                agent: leon_core::AgentId,
                cwd: String,
                title: &str,
                model: &str,
                started: Duration,
                messages: Vec<(&str, &str)>| {
        let started_at = now - started;
        let transcript: Vec<NewMessage> = messages
            .into_iter()
            .enumerate()
            .map(|(index, (role, text))| NewMessage {
                role: match role {
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "tool" => Role::Tool,
                    _ => Role::System,
                },
                text: text.to_owned(),
                at: started_at + Duration::minutes(index as i64 * 3),
            })
            .collect();
        let updated_at = transcript
            .last()
            .map(|message| message.at)
            .unwrap_or(started_at);
        store
            .upsert_session(
                &NewSession {
                    agent,
                    external_id: external.to_owned(),
                    machine_id: local.clone(),
                    cwd,
                    title: title.to_owned(),
                    model: Some(model.to_owned()),
                    started_at,
                    updated_at,
                },
                &transcript,
            )
            .expect("session");
    };

    let claude = leon_core::AgentId::CLAUDE;
    let codex = leon_core::AgentId::CODEX;
    let opencode = leon_core::AgentId::OPENCODE;

    seed(
        "0f6a1e5c-2b7d-4f1a-9c3e-8d2a5b7e4c10",
        claude,
        format!("{home}/code/atlas"),
        "Add rate limiting to the public endpoints",
        "claude-sonnet-4-6",
        Duration::hours(2),
        vec![
            ("user", "add rate limiting to the public endpoints"),
            ("assistant", "Looking at the router and the store first."),
            ("tool", "Read(src/routes.rs) — 64 lines"),
            ("assistant", "The public routes have no limiter. I will put a token bucket in front of them and cover it with a test."),
            ("tool", "Update(src/routes.rs) — 26 additions, 3 removals"),
            ("tool", "Write(tests/rate_limit.rs) — 38 lines"),
            ("assistant", "Done. Public endpoints now allow 100 requests a minute per key, with a 429 and Retry-After when the bucket is empty."),
            ("user", "make the limit configurable per plan"),
        ],
    );

    seed(
        "a41c9d7e-5e2b-4a8f-b6d1-3c7f9e0a2d44",
        claude,
        format!("{home}/code/atlas-login-timeout"),
        "Close idle sessions after 30 minutes",
        "claude-sonnet-4-6",
        Duration::hours(5),
        vec![
            ("user", "sessions stay open forever; close them after 30 idle minutes"),
            ("assistant", "I will add an idle deadline to the session and sweep it from the heartbeat loop."),
            ("tool", "Read(src/session.rs) — 12 lines"),
            ("assistant", "IDLE_TIMEOUT is there but nothing reads it. Wiring it into the sweep now."),
            ("tool", "Update(src/session.rs) — 41 additions"),
            ("assistant", "Sessions now close after 30 idle minutes; a resumed request refreshes the deadline."),
        ],
    );

    seed(
        "c9d3b1a8-7f4e-42b5-9a0c-1e6d8b2f3a55",
        codex,
        format!("{home}/code/atlas-rate-limits"),
        "Review the token bucket math",
        "gpt-5.3-codex",
        Duration::days(1),
        vec![
            ("user", "review the token bucket math in limiter.rs"),
            ("assistant", "The refill divides by the per-request interval; that is correct, but the bucket caps at 100 tokens while a new key starts full."),
            ("tool", "Read(src/limiter.rs) — 44 lines"),
            ("assistant", "Two notes: start the bucket empty to avoid a burst on first use, and use f64 tokens as you already do. The elapsed-time refill is sound."),
        ],
    );

    seed(
        "b7e2f4c6-8a1d-4c3b-95e7-2f0a6d9c1b33",
        claude,
        format!("{home}/code/atlas"),
        "Move usage reports behind the admin key",
        "claude-sonnet-4-6",
        Duration::days(1) + Duration::hours(3),
        vec![
            (
                "user",
                "usage reports should need the admin key, not any key",
            ),
            (
                "assistant",
                "Right now /v1/usage only checks that a key exists.",
            ),
            ("tool", "Update(src/routes.rs) — 9 additions, 4 removals"),
            (
                "assistant",
                "The route now rejects non-admin keys with 403 and the test covers both sides.",
            ),
        ],
    );

    seed(
        "2d8c4e6a-3f9b-4e7d-8a1c-5b0e7f2d9c66",
        opencode,
        format!("{home}/code/ledger"),
        "Add pagination to the entries endpoint",
        "claude-sonnet-4-6",
        Duration::hours(4),
        vec![
            (
                "user",
                "the entries endpoint returns everything; add cursor pagination",
            ),
            (
                "assistant",
                "I will add a cursor on (at, id) so the order stays stable.",
            ),
            ("tool", "Read(src/entries.ts) — 18 lines"),
            ("tool", "Update(src/entries.ts) — 22 additions"),
            (
                "assistant",
                "GET /entries now takes ?after=<cursor>&limit=50 and returns next_cursor.",
            ),
        ],
    );

    seed(
        "e5f7a9c1-6b2d-4f8e-a3c5-9d1b4e7f0a88",
        opencode,
        format!("{home}/code/ledger-csv-export"),
        "Escape commas in the CSV export",
        "claude-sonnet-4-6",
        Duration::minutes(30),
        vec![
            ("user", "account names with commas break the csv export"),
            (
                "assistant",
                "Quoting fields that contain a comma, a quote or a newline, doubling inner quotes.",
            ),
            ("tool", "Update(src/csv.ts) — 14 additions, 3 removals"),
            (
                "assistant",
                "toCsv now escapes every field; the test covers a name like \"ACME, Inc.\"",
            ),
        ],
    );

    seed(
        "7a1b3c5d-9e2f-4a6b-8c0d-1e3f5a7b9c22",
        opencode,
        format!("{home}/code/ledger"),
        "Explain the balance rounding rules",
        "claude-sonnet-4-6",
        Duration::days(2),
        vec![
            ("user", "why do balances sometimes differ by a cent?"),
            ("assistant", "Amounts are stored as floats; the report sums them and rounds once at the end, but each entry is rounded when written."),
            ("assistant", "Store minor units (integers) instead, and format only for display."),
        ],
    );

    println!(
        "seeded {} projects and 7 sessions into {}",
        store.projects(None).expect("projects").len(),
        data_dir
    );
}

fn worktree(path: &str, branch: &str, is_main: bool) -> NewWorktree {
    NewWorktree {
        path: path.to_owned(),
        branch: Some(branch.to_owned()),
        head: None,
        is_main,
    }
}
