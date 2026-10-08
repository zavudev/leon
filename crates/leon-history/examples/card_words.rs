//! Prints what the Den's card would quote from a transcript: every prompt
//! the user is said to have typed and what every call asks.
//!
//! ```sh
//! cargo run -p leon-history --example card_words -- <claude|codex> <file>
//! ```
//!
//! A check for the readers against real transcripts: nothing the harness
//! wrote must show as something the user said. Each text is cut to its
//! first 110 characters and its line breaks are shown as `\n`.

use leon_history::live::{Beat, Follower, Format, Start};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(format), Some(path)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: card_words <claude|codex> <file>");
        std::process::exit(2);
    };
    let format = match format.as_str() {
        "claude" => Format::Claude,
        "codex" => Format::Codex,
        other => {
            eprintln!("unknown format {other}");
            std::process::exit(2);
        }
    };
    let mut follower = Follower::new(&path, format, Start::Beginning);
    let polled = match follower.poll() {
        Ok(polled) => polled,
        Err(error) => {
            eprintln!("cannot read {path}: {error}");
            std::process::exit(1);
        }
    };
    let cut = |text: &str| {
        let line = text.replace('\n', "\\n");
        let head: String = line.chars().take(110).collect();
        format!("{head} [{} chars]", text.chars().count())
    };
    for beat in &polled.beats {
        match beat {
            Beat::Heard { text, .. } => println!("HEARD  {}", cut(text)),
            Beat::Brief { text, options, .. } => {
                println!("BRIEF  {}", cut(text));
                if !options.is_empty() {
                    println!("       answers: {}", options.join(" | "));
                }
            }
            _ => {}
        }
    }
}
