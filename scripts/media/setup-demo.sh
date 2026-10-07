#!/usr/bin/env bash
# Builds the demo workspace for Leon's README media: two small git projects
# with a few worktrees, a shell rc and the simulated agents. Everything
# lives under the demo home.
#
#   scripts/media/setup-demo.sh [demo-home]
#
# The home is mounted at /home/ada by launch.sh; running this from inside
# that mount keeps the worktree metadata spelled the way Leon will see it.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
HOME_DIR="${1:-/tmp/leon-media/home}"
CODE="$HOME_DIR/code"

rm -rf "$CODE"
mkdir -p "$CODE" "$HOME_DIR/bin"

cp "$HERE/home/bashrc" "$HOME_DIR/.bashrc"
cp "$HERE/mock-agents/"* "$HOME_DIR/bin/"
chmod +x "$HOME_DIR/bin/"*

git_init() {
    local dir="$1"
    git -C "$dir" init -q -b main
    git -C "$dir" config user.name "Ada Lovelace"
    git -C "$dir" config user.email "ada@example.com"
    git -C "$dir" config commit.gpgsign false
}

commit_all() {
    local dir="$1" message="$2"
    git -C "$dir" add -A
    git -C "$dir" commit -q -m "$message"
}

# ---------------------------------------------------------------- atlas -----
ATLAS="$CODE/atlas"
mkdir -p "$ATLAS/src" "$ATLAS/tests"
git_init "$ATLAS"

cat > "$ATLAS/README.md" <<'EOF'
# atlas

The public API of Atlas: keys, quotas and usage reports.
EOF

cat > "$ATLAS/Cargo.toml" <<'EOF'
[package]
name = "atlas"
version = "0.7.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
EOF

cat > "$ATLAS/src/main.rs" <<'EOF'
mod routes;
mod store;

#[tokio::main]
async fn main() {
    let store = store::open("atlas.db").expect("store");
    let app = routes::router(store);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
EOF

cat > "$ATLAS/src/routes.rs" <<'EOF'
use axum::{routing::get, Router};

use crate::store::Store;

pub fn router(store: Store) -> Router {
    Router::new()
        .route("/v1/keys", get(list_keys))
        .route("/v1/usage", get(usage))
        .with_state(store)
}

async fn list_keys() -> &'static str {
    "[]"
}

async fn usage() -> &'static str {
    "{}"
}
EOF

cat > "$ATLAS/src/store.rs" <<'EOF'
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Store {
    keys: Arc<Mutex<Vec<String>>>,
}

pub fn open(_path: &str) -> Result<Store, std::io::Error> {
    Ok(Store {
        keys: Arc::new(Mutex::new(Vec::new())),
    })
}
EOF

cat > "$ATLAS/tests/keys.rs" <<'EOF'
#[test]
fn the_key_list_starts_empty() {
    assert!(true);
}
EOF

commit_all "$ATLAS" "Start the Atlas API"

# rate-limits: this branch lands, so its worktree shows as merged.
git -C "$ATLAS" worktree add -q "$CODE/atlas-rate-limits" -b rate-limits
cat > "$CODE/atlas-rate-limits/src/limiter.rs" <<'EOF'
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A token bucket per API key: 100 requests a minute.
pub struct Limiter {
    buckets: HashMap<String, Bucket>,
    rate: Duration,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Limiter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            buckets: HashMap::new(),
            rate: Duration::from_secs(60) / per_minute,
        }
    }

    pub fn allow(&mut self, key: &str) -> bool {
        let bucket = self.buckets.entry(key.to_owned()).or_insert(Bucket {
            tokens: 100.0,
            last: Instant::now(),
        });
        let elapsed = bucket.last.elapsed();
        bucket.tokens = (bucket.tokens + elapsed.as_secs_f64() / self.rate.as_secs_f64()).min(100.0);
        bucket.last = Instant::now();
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
EOF
commit_all "$CODE/atlas-rate-limits" "Add a token bucket limiter"

# login-timeout: still open, the worktree the demo agent works in.
git -C "$ATLAS" worktree add -q "$CODE/atlas-login-timeout" -b login-timeout
cat > "$CODE/atlas-login-timeout/src/session.rs" <<'EOF'
use std::time::Duration;

/// How long a session may sit idle before it is closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
EOF
commit_all "$CODE/atlas-login-timeout" "Start session handling"

git -C "$ATLAS" merge -q --no-ff --no-edit rate-limits
git -C "$ATLAS" worktree list | head -3

# --------------------------------------------------------------- ledger -----
LEDGER="$CODE/ledger"
mkdir -p "$LEDGER/src"
git_init "$LEDGER"

cat > "$LEDGER/README.md" <<'EOF'
# ledger

Double-entry bookkeeping for small teams: entries, accounts and reports.
EOF

cat > "$LEDGER/package.json" <<'EOF'
{
  "name": "ledger",
  "version": "1.2.0",
  "type": "module",
  "scripts": { "test": "node --test" }
}
EOF

cat > "$LEDGER/src/entries.ts" <<'EOF'
export interface Entry {
  id: string;
  account: string;
  amount: number;
  currency: string;
  at: string;
}

export function balance(entries: Entry[], account: string): number {
  return entries
    .filter((entry) => entry.account === account)
    .reduce((sum, entry) => sum + entry.amount, 0);
}
EOF

cat > "$LEDGER/src/server.ts" <<'EOF'
import { balance } from "./entries.js";

export function entriesReport(entries: Parameters<typeof balance>[0]) {
  return { count: entries.length, total: entries.reduce((s, e) => s + e.amount, 0) };
}
EOF

commit_all "$LEDGER" "Start the ledger service"

git -C "$LEDGER" worktree add -q "$CODE/ledger-csv-export" -b csv-export
cat > "$CODE/ledger-csv-export/src/csv.ts" <<'EOF'
import type { Entry } from "./entries.js";

export function toCsv(entries: Entry[]): string {
  const header = "id,account,amount,currency,at";
  const rows = entries.map(
    (entry) => `${entry.id},${entry.account},${entry.amount},${entry.currency},${entry.at}`,
  );
  return [header, ...rows].join("\n");
}
EOF
commit_all "$CODE/ledger-csv-export" "Add CSV export"

echo "demo workspace ready at $CODE"
