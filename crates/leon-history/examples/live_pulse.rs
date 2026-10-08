//! Reads a transcript the way the live view does and prints what it found.
//!
//! ```sh
//! cargo run -p leon-history --example live_pulse -- <claude|claude-subagent|codex> <file>
//! ```
//!
//! Prints how many beats of each kind the file holds, which tools were
//! called and as what kind, how long reading it took, and the picture at
//! its end. Tool details are printed, message text
//! never is (it is never read).

use std::collections::BTreeMap;
use std::time::Instant;

use leon_history::live::{Beat, Follower, Format, Pulse, Start};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(format), Some(path)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: live_pulse <claude|claude-subagent|codex> <file>");
        std::process::exit(2);
    };
    let format = match format.as_str() {
        "claude" => Format::Claude,
        "claude-subagent" => Format::ClaudeSubagent,
        "codex" => Format::Codex,
        other => {
            eprintln!("unknown format {other}");
            std::process::exit(2);
        }
    };

    let started = Instant::now();
    let mut follower = Follower::new(&path, format, Start::Beginning);
    let polled = match follower.poll() {
        Ok(polled) => polled,
        Err(error) => {
            eprintln!("cannot read {path}: {error}");
            std::process::exit(1);
        }
    };
    let elapsed = started.elapsed();

    let mut pulse = Pulse::new();
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
    for beat in &polled.beats {
        pulse.apply(beat);
        *counts.entry(name(beat)).or_default() += 1;
        if let Beat::ToolStarted { name, kind, .. } = beat {
            let tool = name.split("__").next().unwrap_or(name);
            *kinds.entry(format!("{kind:?} ({tool})")).or_default() += 1;
        }
    }

    println!(
        "{} bytes, {} beats, {} unreadable lines, {elapsed:?}",
        follower.offset(),
        polled.beats.len(),
        follower.tail().malformed()
    );
    for (name, count) in counts {
        println!("  {name:<13} {count}");
    }
    for (kind, count) in kinds {
        println!("  tool {kind:<28} {count}");
    }
    println!(
        "phase {:?}, turn over {}, {} tool calls ({} failed), {} compactions, context {:?} of {:?}",
        pulse.phase(),
        pulse.turn_over(),
        pulse.tool_calls(),
        pulse.failed_calls(),
        pulse.compactions(),
        pulse.context_tokens(),
        pulse.context_window()
    );
    for tool in pulse.tools() {
        println!(
            "  in flight: {} ({:?}) {}",
            tool.name, tool.kind, tool.detail
        );
    }
    for agent in pulse.subagents() {
        println!(
            "  sub-agent: {} id {:?} background {}",
            agent.label, agent.agent, agent.background
        );
    }
}

fn name(beat: &Beat) -> &'static str {
    match beat {
        Beat::Prompt => "Prompt",
        Beat::Heard { .. } => "Heard",
        Beat::Brief { .. } => "Brief",
        Beat::Woken => "Woken",
        Beat::Thinking => "Thinking",
        Beat::Said { .. } => "Said",
        Beat::ToolStarted { .. } => "ToolStarted",
        Beat::ToolFinished { .. } => "ToolFinished",
        Beat::Detached { .. } => "Detached",
        Beat::TaskEnded { .. } => "TaskEnded",
        Beat::TasksKilled => "TasksKilled",
        Beat::TurnEnded => "TurnEnded",
        Beat::Interrupted => "Interrupted",
        Beat::Compacted { .. } => "Compacted",
        Beat::Usage { .. } => "Usage",
    }
}
