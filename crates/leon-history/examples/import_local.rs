//! Manual sanity check: imports this machine's real agent history into a
//! throw-away database and prints counts and timings.
//!
//! Nothing is written outside a temporary directory, the agents' own stores
//! are only read, and no message content, title or path from a session is
//! printed. Run with:
//!
//! ```text
//! cargo run --release -p leon-history --example import_local
//! ```

use std::time::Instant;

use leon_core::{MachineId, SearchQuery, SessionFilter, Store};
use leon_history::{default_roots, Importer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let roots = default_roots();
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("leon.db"))?;
    let machine = MachineId::local();

    let started = Instant::now();
    let first = Importer::run(&store, &machine, &roots);
    let first_elapsed = started.elapsed();
    println!("first run:  {first:?}");
    println!("            took {first_elapsed:.2?}");

    let started = Instant::now();
    let second = Importer::run(&store, &machine, &roots);
    println!("second run: {second:?}");
    println!("            took {:.2?}", started.elapsed());

    for agent in leon_core::agent::builtin()
        .iter()
        .filter(|spec| spec.history.is_some())
        .map(|spec| spec.id)
    {
        let filter = SessionFilter {
            agent: Some(agent),
            ..Default::default()
        };
        let sessions = store.recent_sessions(&filter, usize::MAX)?;
        let messages: u64 = sessions
            .iter()
            .map(|session| u64::from(session.message_count))
            .sum();
        println!(
            "{:<9} {:>6} sessions {:>8} messages",
            agent.as_str(),
            sessions.len(),
            messages
        );
    }

    for word in ["error", "test", "refactor the"] {
        let started = Instant::now();
        let hits = store.search(&SearchQuery::new(word))?;
        println!(
            "search {word:?}: {} hits in {:.2?}",
            hits.len(),
            started.elapsed()
        );
    }

    let size = std::fs::metadata(directory.path().join("leon.db"))?.len();
    println!("database size: {:.1} MiB", size as f64 / (1024.0 * 1024.0));
    Ok(())
}
