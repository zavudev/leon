//! Paints the Den into PNG files, with no window: to look at the art.
//!
//! ```text
//! cargo run -p leon-den --example den_png -- OUT_DIR
//! ```
//!
//! It writes `den-<name>-dark.png` and `den-<name>-light.png`: every prefab
//! with lions in it, at a moment of the demo's script, and the feed long,
//! gone back in, plain, and in a narrow and a small view. The agents'
//! messages are made up. The room is the picture the GPUI
//! view paints ([`leon_den::paint::compose`]) and the chrome is the same
//! scene ([`leon_den::scene::build`]); only the text differs: it is set here
//! in the Den's own little font, since there is no text system.
//!
//! With a second argument `crowd` it writes instead
//! `crowd-<prefab>-<count>-<mix>.png`: every prefab with 8, 16 and 24 lions
//! that carry long real-looking titles, in mixed states and all in one
//! (asleep, waiting, reading), settled: to look at where everybody ends up.

#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use leon_den::bitmap::Bitmap;
use leon_den::glyphs;
use leon_den::paint::{compose, Wardrobe};
use leon_den::palette::bytes;
use leon_den::scene::{build, Layout, TextMetrics};
use leon_den::Den;

fn write(out: &mut Bitmap, x: i32, y: i32, unit: i32, text: &str, color: [u8; 4]) {
    for (index, letter) in text.chars().enumerate() {
        for (row, bits) in glyphs::glyph(letter).iter().enumerate() {
            for col in 0..3 {
                if bits >> (2 - col) & 1 == 1 {
                    let left = x + (index as i32 * 4 + col) * unit;
                    out.fill(left, y + row as i32 * unit, unit, unit, color);
                }
            }
        }
    }
}

struct Shot {
    /// The name of the file, after `den-`.
    name: &'static str,
    /// Which lion of the roster is selected, if one is.
    select: Option<usize>,
    /// How many pages back the feed is scrolled.
    back: i32,
    /// The narrator is off.
    plain: bool,
    prefab: &'static str,
    size: (i32, i32),
    lions: usize,
    rounds: usize,
    reduced: bool,
}

fn den(out: &str, dark: bool, shot: &Shot) {
    let palette = support::palette(dark);
    let metrics = TextMetrics {
        char_w: 8.,
        line_h: 18.,
    };
    let (width, height) = shot.size;
    let room = leon_den::prefabs::prefab(shot.prefab)
        .expect("a prefab")
        .layout;
    let mut den = Den::new();
    let mut now = Duration::ZERO;
    den.set_reduced_motion(shot.reduced, now);
    den.set_layout(&room, false, now);
    den.set_plain_status(shot.plain);
    // A made-up afternoon: the time is data, as it is for the view.
    let mut wall: i64 = 1_791_400_000;
    let mut before: Vec<leon_den::Cub> = Vec::new();
    for round in 0..=shot.rounds {
        den.set_wall_time(wall);
        let cubs = support::cast(dark, shot.lions, |_| round);
        den.update(&cubs, now);
        if round == 0 {
            // What the sessions did before the Den was opened.
            for (index, cub) in cubs.iter().filter(|cub| cub.parent.is_none()).enumerate() {
                den.remember(cub.id, &cub.name, &support::past(index, wall));
            }
        }
        for happening in support::happenings(&before, &cubs, round) {
            den.happen(&happening, now);
        }
        for (cub, name, text) in support::speeches(&before, &cubs, round) {
            den.speak(cub, &name, &text, None, now);
        }
        before = cubs;
        now += Duration::from_millis(9000);
        wall += 95;
    }
    // A moment into the last round.
    now -= Duration::from_millis(1200);
    let order = den.order();
    den.select(shot.select.and_then(|nth| order.get(nth).copied()));
    let lions = order.len();
    let layout = Layout::compute(width, height, 1., metrics, room.cols, room.rows, lions);
    let frame = den.frame(now);
    if shot.back > 0 {
        // The reader went back: the feed stays there.
        let scene = build(&den, &frame, &layout, &palette, None);
        let (total, rows) = (scene.feed_lines.len(), scene.feed_height);
        den.feed_mut()
            .scroll(-i64::from(shot.back) * rows as i64, total, rows);
        den.speak(
            1,
            "moss",
            "One more thing: the changelog entry is missing.",
            None,
            now,
        );
    }
    let scene = build(&den, &frame, &layout, &palette, None);
    let mut wardrobe = Wardrobe::new();
    let picture = compose(&den, &frame, &palette, &mut wardrobe, None).scaled(layout.unit);

    let mut canvas = Bitmap::new(width, height);
    for quad in &scene.under {
        canvas.fill(
            quad.rect.x,
            quad.rect.y,
            quad.rect.w,
            quad.rect.h,
            bytes(quad.color),
        );
    }
    canvas.draw(&picture, scene.room.x, scene.room.y);
    for quad in &scene.quads {
        canvas.fill(
            quad.rect.x,
            quad.rect.y,
            quad.rect.w,
            quad.rect.h,
            bytes(quad.color),
        );
    }
    for text in &scene.texts {
        write(
            &mut canvas,
            text.x,
            text.y + 3,
            2,
            &text.text,
            bytes(text.color),
        );
    }
    let name = if dark { "dark" } else { "light" };
    let path = format!("{out}/den-{}-{name}.png", shot.name);
    std::fs::write(&path, canvas.to_png()).expect("the PNG is written");
    println!(
        "{path}: {}x{} tiles, unit {}, missing {:?}",
        layout.cols,
        layout.rows,
        layout.unit,
        den.world().missing()
    );
}

/// The names real sessions have: long, in Spanish, with accents.
const TITLES: [&str; 8] = [
    "Lineamientos de voz y personalidad de marca",
    "Chat móvil para el relay",
    "MCP para Fudo v3 y WhatsApp",
    "Otro PR",
    "WhatsApp embedded signup",
    "Leon integration",
    "Mirada HA",
    "Opción para pinear sesiones",
];

/// A crowd in a room, everybody settled: the room alone, with no chrome.
fn crowd(out: &str, prefab: &leon_den::prefabs::Prefab, count: usize, mix: &str) {
    use leon_den::CubState as S;
    let states: Vec<S> = match mix {
        "asleep" => vec![S::Asleep],
        "waiting" => vec![S::WaitingForUser],
        "reading" => vec![S::Reading],
        "owner" => vec![
            S::Asleep,
            S::Asleep,
            S::Asleep,
            S::WaitingForUser,
            S::Asleep,
            S::Asleep,
            S::WaitingForUser,
            S::Asleep,
        ],
        _ => vec![
            S::Editing,
            S::Asleep,
            S::Reading,
            S::WaitingForUser,
            S::Idle,
            S::Running,
            S::Thinking,
            S::NeedsPermission,
            S::Web,
            S::Planning,
            S::Searching,
            S::Mystery,
        ],
    };
    let palette = support::palette(true);
    let tints = support::tints(true);
    let mut den = Den::new();
    den.set_layout(&prefab.layout, false, Duration::ZERO);
    let cubs: Vec<leon_den::Cub> = (0..count)
        .map(|index| leon_den::Cub {
            id: index as u64 + 1,
            name: TITLES[index % TITLES.len()].to_owned(),
            species: leon_den::Species {
                tint: tints[index % 3],
                seed: index as u64 * 7 + 3,
            },
            state: states[index % states.len()],
            level: 3,
            detail: None,
            parent: None,
            mystery: false,
        })
        .collect();
    den.update(&cubs, Duration::ZERO);
    let frame = den.frame(Duration::from_secs(600));
    let picture = compose(&den, &frame, &palette, &mut Wardrobe::new(), None).scaled(3);
    let path = format!("{out}/crowd-{}-{count:02}-{mix}.png", prefab.id);
    std::fs::write(&path, picture.to_png()).expect("the PNG is written");
    let named = leon_den::paint::plates(&frame, picture.w / 3, picture.h / 3).len();
    println!("{path}: {named} of {count} named");
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    if std::env::args().nth(2).as_deref() == Some("crowd") {
        for prefab in leon_den::prefabs::prefabs() {
            for count in [8, 16, 24] {
                for mix in ["mixed", "asleep", "waiting", "reading", "owner"] {
                    crowd(&out, &prefab, count, mix);
                }
            }
        }
        return;
    }
    let shots = [
        Shot {
            name: "office",
            select: Some(1),
            back: 0,
            plain: false,
            prefab: "office",
            size: (1280, 800),
            lions: 6,
            rounds: 8,
            reduced: false,
        },
        Shot {
            name: "open-plan",
            select: None,
            back: 0,
            plain: false,
            prefab: "open-plan",
            size: (1600, 1000),
            lions: 16,
            rounds: 11,
            reduced: true,
        },
        Shot {
            name: "library",
            select: Some(0),
            back: 0,
            plain: false,
            prefab: "library",
            size: (1280, 800),
            lions: 7,
            rounds: 15,
            reduced: false,
        },
        Shot {
            name: "server-room",
            select: None,
            back: 0,
            plain: false,
            prefab: "server-room",
            size: (1280, 800),
            lions: 6,
            rounds: 3,
            reduced: false,
        },
        Shot {
            name: "lounge",
            select: None,
            back: 0,
            plain: false,
            prefab: "lounge",
            size: (1280, 800),
            lions: 6,
            rounds: 13,
            reduced: false,
        },
        Shot {
            name: "nook",
            select: None,
            back: 0,
            plain: false,
            prefab: "nook",
            size: (1000, 760),
            lions: 2,
            rounds: 2,
            reduced: false,
        },
        // The feed: long, of everybody; gone back in; plain; and under the
        // room in a narrow pane.
        Shot {
            name: "feed-all",
            select: None,
            back: 0,
            plain: false,
            prefab: "office",
            size: (1440, 900),
            lions: 6,
            rounds: 14,
            reduced: false,
        },
        Shot {
            name: "feed-back",
            select: None,
            back: 2,
            plain: false,
            prefab: "office",
            size: (1440, 900),
            lions: 6,
            rounds: 14,
            reduced: true,
        },
        Shot {
            name: "feed-plain",
            select: None,
            back: 0,
            plain: true,
            prefab: "office",
            size: (1440, 900),
            lions: 6,
            rounds: 14,
            reduced: true,
        },
        Shot {
            name: "feed-narrow",
            select: None,
            back: 0,
            plain: false,
            prefab: "office",
            size: (700, 860),
            lions: 4,
            rounds: 9,
            reduced: true,
        },
        Shot {
            name: "feed-small",
            select: None,
            back: 0,
            plain: false,
            prefab: "nook",
            size: (520, 360),
            lions: 2,
            rounds: 3,
            reduced: true,
        },
    ];
    for dark in [true, false] {
        for shot in &shots {
            den(&out, dark, shot);
        }
    }
}
