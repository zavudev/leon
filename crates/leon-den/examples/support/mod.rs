//! What the two examples share: the tokens of the Leon themes, written out
//! here because an example has no theme to read them from.

use gpui_kit::{rgb, Hsla};
use leon_den::palette::{DenPalette, Tokens};

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// The tokens of the Leon theme, dark or light (`brand/tokens.md`).
pub fn tokens(dark: bool) -> Tokens {
    if dark {
        Tokens {
            background: hex(0x0a0a0a),
            surface: hex(0x141414),
            surface_2: hex(0x1c1c1c),
            border: hex(0x262626),
            guide: hex(0x333333),
            grid_mark: hex(0x555555),
            text: hex(0xfafaf9),
            text_muted: hex(0xa8a29e),
            text_faint: hex(0x8c8580),
            signal: hex(0xffea00),
            accent_fill: hex(0xffea00),
            on_accent_fill: hex(0x0a0a0a),
            success: hex(0x4df688),
            warning: hex(0xff9500),
            error: hex(0xff5e5e),
            info: hex(0x6efaff),
        }
    } else {
        Tokens {
            background: hex(0xfafaf9),
            surface: hex(0xffffff),
            surface_2: hex(0xf5f5f4),
            border: hex(0xe7e5e4),
            guide: hex(0xd3d1d0),
            grid_mark: hex(0xaaa8a7),
            text: hex(0x0c0a09),
            text_muted: hex(0x57534e),
            text_faint: hex(0x78716c),
            signal: hex(0x756600),
            accent_fill: hex(0xffea00),
            on_accent_fill: hex(0x0a0a0a),
            success: hex(0x047857),
            warning: hex(0xa06000),
            error: hex(0xb91c1c),
            info: hex(0x0e7490),
        }
    }
}

/// The palette of the den in the Leon theme.
pub fn palette(dark: bool) -> DenPalette {
    DenPalette::from_tokens(&tokens(dark))
}

/// The mane tints of the three agents, dark or light.
pub fn tints(dark: bool) -> [Hsla; 3] {
    if dark {
        [hex(0xd97757), hex(0xfafaf9), hex(0xf1ecec)]
    } else {
        [hex(0xb8583a), hex(0x0c0a09), hex(0x211e1e)]
    }
}

use leon_den::model::{Cub, CubState, Event, Happening, Species, ToolKind};

const NAMES: [&str; 16] = [
    "moss", "fern", "juniper", "ash", "wren", "pike", "sol", "tansy", "brook", "flint", "hazel",
    "rowan", "sage", "thorn", "vale", "yarrow",
];

/// What a lion of the demo goes through, step after step.
const SCRIPT: [CubState; 18] = [
    CubState::Thinking,
    CubState::Reading,
    CubState::Editing,
    CubState::Running,
    CubState::Editing,
    CubState::Searching,
    CubState::Web,
    CubState::Planning,
    CubState::Delegating,
    CubState::Delegating,
    CubState::NeedsPermission,
    CubState::Editing,
    CubState::WaitingForUser,
    CubState::Idle,
    CubState::Asleep,
    CubState::Mystery,
    CubState::Editing,
    CubState::Fainted,
];

const FILES: [&str; 6] = [
    "crates/app/src/ui/shell.rs",
    "keys.rs",
    "docs/ARCHITECTURE.md",
    "crates/leon-term/src/view.rs",
    "Cargo.toml",
    "theme/palette.rs",
];
const COMMANDS: [&str; 4] = [
    "cargo test -p leon-den",
    "cargo clippy --all-targets -- -D warnings",
    "git status --short",
    "npm run build",
];

fn detail(state: CubState, pick: usize) -> Option<String> {
    let file = FILES[pick % FILES.len()];
    Some(match state {
        CubState::Editing => format!("Editing {file}"),
        CubState::Reading => format!("Reading {file}"),
        CubState::Searching => "Searching for \"Overlay\"".to_owned(),
        CubState::Running => COMMANDS[pick % COMMANDS.len()].to_owned(),
        CubState::Web => "https://docs.rs/gpui".to_owned(),
        CubState::Delegating => "Task: explore the terminal crate".to_owned(),
        _ => return None,
    })
}

/// The lions of the demo: `count` of them, the lion `index` at the step
/// `steps(index)` of the script, and a little one for each that is
/// delegating.
pub fn cast(dark: bool, count: usize, steps: impl Fn(usize) -> usize) -> Vec<Cub> {
    let tints = tints(dark);
    let mut cubs = Vec::new();
    for index in 0..count {
        let step = steps(index);
        let at = (index * 5 + step) % SCRIPT.len();
        let state = SCRIPT[at];
        let name = NAMES[index % NAMES.len()];
        cubs.push(Cub {
            id: index as u64 + 1,
            name: name.to_owned(),
            species: Species {
                tint: tints[index % 3],
                seed: index as u64 * 7 + 3,
            },
            state,
            level: (step * 2 + index * 3) as u32,
            detail: detail(state, index + step),
            parent: None,
            mystery: state == CubState::Mystery,
        });
        if state == CubState::Delegating {
            let first = SCRIPT[(at + 1) % SCRIPT.len()] == CubState::Delegating;
            cubs.push(Cub {
                id: 100 + index as u64,
                name: "explore".to_owned(),
                species: Species {
                    tint: tints[index % 3],
                    seed: index as u64 + 40,
                },
                state: if first {
                    CubState::Reading
                } else {
                    CubState::Editing
                },
                level: if first { 2 } else { 6 },
                detail: Some("Reading crates/leon-term/src/keys.rs".to_owned()),
                parent: Some(index as u64 + 1),
                mystery: false,
            });
        }
    }
    cubs
}

/// What happened between two rounds of the demo, as the app would tell it.
pub fn happenings(before: &[Cub], after: &[Cub], round: usize) -> Vec<Happening> {
    let mut out = Vec::new();
    for cub in after {
        let old = before.iter().find(|old| old.id == cub.id);
        let tell = |event: Event| Happening::new(cub.id, cub.name.clone(), event);
        let Some(old) = old else {
            if cub.parent.is_none() {
                out.push(tell(Event::Joined));
            }
            continue;
        };
        if old.state == cub.state {
            continue;
        }
        if old.state == CubState::Running {
            out.push(tell(Event::ToolFinished {
                kind: ToolKind::Run,
                ok: (round + cub.id as usize) % 3 != 0,
            }));
        }
        let subject = |prefix: &str| {
            cub.detail
                .as_deref()
                .map(|detail| detail.trim_start_matches(prefix).to_owned())
        };
        let tool = |kind: ToolKind, tool: &str, detail: Option<String>| Event::ToolStarted {
            kind,
            tool: tool.to_owned(),
            detail,
        };
        out.push(tell(match cub.state {
            CubState::Editing => tool(ToolKind::Edit, "Edit", subject("Editing ")),
            CubState::Reading => tool(ToolKind::Read, "Read", subject("Reading ")),
            CubState::Searching => tool(ToolKind::Search, "Grep", Some("Overlay".to_owned())),
            CubState::Running => tool(ToolKind::Run, "Bash", cub.detail.clone()),
            CubState::Web => tool(ToolKind::Web, "WebFetch", Some("docs.rs/gpui".to_owned())),
            CubState::Planning => tool(ToolKind::Plan, "TodoWrite", None),
            CubState::Delegating => Event::SentOut {
                little: "explore".to_owned(),
            },
            CubState::NeedsPermission => Event::PermissionPrompt,
            CubState::WaitingForUser => Event::TurnEnded,
            CubState::Asleep => Event::FellAsleep,
            CubState::Mystery => Event::Mysterious,
            CubState::Fainted => Event::Fainted { exit: Some(101) },
            CubState::Gone => Event::WentHome,
            CubState::Thinking | CubState::Idle | CubState::UsingTool => continue,
        }));
    }
    for old in before {
        if old.parent.is_some() && !after.iter().any(|cub| cub.id == old.id) {
            if let Some(parent) = after.iter().find(|cub| Some(cub.id) == old.parent) {
                out.push(Happening::new(
                    parent.id,
                    parent.name.clone(),
                    Event::CameBack {
                        little: old.name.clone(),
                    },
                ));
            }
        }
    }
    out
}

/// What the agents of the demo say to the user: made-up messages, of the
/// lengths real ones have.
const SPEECH: [&str; 6] = [
    "Done. The failing test was comparing against a stale fixture; I regenerated it and the suite passes.",
    "I found two call sites that still use the old signature:\n\n- `crates/app/src/ui/shell.rs`\n- `crates/app/src/keys.rs`\n\nI'll update both and run the tests again before touching anything else. If the palette test fails after that, it is because the registry lists the command twice, and I will fix the registry rather than the test.",
    "Which of the two do you want: keep the chord and move the menu entry, or the other way round?",
    "The build is green.",
    "I need to run `cargo test -p leon` to be sure. It takes about a minute.",
    "Here is the plan:\n\n1. Read the settings schema.\n2. Add the key.\n3. Regenerate the reference.\n4. Run the gate.",
];

/// What the lions whose turn just ended said: `(cub, name, text)`.
pub fn speeches(before: &[Cub], after: &[Cub], round: usize) -> Vec<(u64, String, String)> {
    after
        .iter()
        .filter(|cub| cub.parent.is_none() && cub.state == CubState::WaitingForUser)
        .filter(|cub| {
            before
                .iter()
                .find(|old| old.id == cub.id)
                .is_some_and(|old| old.state != cub.state)
        })
        .map(|cub| {
            let text = SPEECH[(round + cub.id as usize) % SPEECH.len()];
            (cub.id, cub.name.clone(), text.to_owned())
        })
        .collect()
}

/// The past of a lion of the demo, as a transcript read from its start
/// would tell it.
pub fn past(index: usize, now: i64) -> Vec<leon_den::feed::Past> {
    use leon_den::feed::Past;
    let mut out = Vec::new();
    for turn in 0..3 {
        for _ in 0..(3 + index * 2 + turn * 4) {
            out.push(Past::Tool);
        }
        out.push(Past::Speech(
            SPEECH[(index + turn) % SPEECH.len()].to_owned(),
            Some(now - 3600 * (3 - turn as i64) - 120 * index as i64),
        ));
        out.push(Past::TurnEnded);
    }
    out
}
