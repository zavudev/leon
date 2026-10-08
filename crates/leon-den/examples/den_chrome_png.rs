//! Paints the chrome of the Den that a host fills in — the truth card with
//! what a lion asks and the summary of its session, the roster with who
//! needs the user and who is at home, the keys over the room — into PNG
//! files, with no window, over both pictures of the room: 2.5D and the pixel
//! art.
//!
//! ```text
//! cargo run -p leon-den --features den3d --example den_chrome_png -- OUT_DIR
//! ```
//!
//! As in `den_view_png`, the boxes are where the view puts them and the
//! letters are the Den's own little font, not the host's. The notes and the
//! keys are written here as a host would hand them in.

#[allow(dead_code)]
#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::time::Duration;

use leon_den::bitmap::Bitmap;
use leon_den::glyphs;
use leon_den::iso::overlay::{box_of, lay, Lettering};
use leon_den::iso::room;
use leon_den::iso::{Camera, Renderer, Theme};
use leon_den::model::{Cub, CubState, Species};
use leon_den::paint::{compose, Wardrobe};
use leon_den::palette::bytes;
use leon_den::scene::{build, build_around, Layout, Rect, TextMetrics};
use leon_den::{Den, HomeEntry, Note};

const UNIT: i32 = 2;

fn write(out: &mut Bitmap, x: i32, y: i32, text: &str, color: [u8; 4]) {
    for (index, letter) in glyphs::plain(text).chars().enumerate() {
        for (row, bits) in glyphs::glyph(letter).iter().enumerate() {
            for col in 0..3 {
                if bits >> (2 - col) & 1 == 1 {
                    let left = x + (index as i32 * 4 + col) * UNIT;
                    out.fill(left, y + row as i32 * UNIT, UNIT, UNIT, color);
                }
            }
        }
    }
}

const CAST: [(&str, CubState); 9] = [
    ("whatsapp", CubState::Editing),
    ("leon", CubState::WaitingForUser),
    ("chat movil", CubState::Running),
    ("mcp", CubState::NeedsPermission),
    ("otro pr", CubState::Thinking),
    ("engram", CubState::Fainted),
    ("omarchy", CubState::Reading),
    ("opcion", CubState::WaitingForUser),
    ("mirada", CubState::Asleep),
];

fn cast(count: usize, dark: bool) -> Vec<Cub> {
    CAST.iter()
        .take(count)
        .enumerate()
        .map(|(index, (name, state))| Cub {
            id: index as u64 + 1,
            name: (*name).to_owned(),
            species: Species {
                tint: support::tints(dark)[index % 3],
                seed: index as u64 * 7 + 3,
            },
            state: *state,
            level: index as u32 * 4 + 2,
            detail: match state {
                CubState::NeedsPermission => Some(
                    "Bash has no result and the terminal is quiet: probably a permission prompt"
                        .to_owned(),
                ),
                CubState::WaitingForUser => Some("Finished its turn".to_owned()),
                _ => Some("crates/app/src/ui/den.rs".to_owned()),
            },
            parent: None,
            mystery: false,
        })
        .collect()
}

/// What a host says of a session that works.
fn summary() -> Vec<Note> {
    vec![
        Note::heading("CONVERSATION"),
        Note::plain("You first said: make the Den follow the brand and the theme"),
        Note::plain("You last said: the default office needs to be bigger"),
        Note::plain("Last said: The office is 20 by 15 tiles now, with twelve seats."),
        Note::plain("7 of your messages, 212 in all"),
        Note::plain("1 message queued for its prompt"),
        Note::heading("SESSION"),
        Note::plain("Claude Code, claude-opus-5-5"),
        Note::plain("In leon, on feat/den-lion-actions"),
        Note::plain("Title: The Den in 2.5D"),
        Note::plain("Running for 2 h 05 min"),
        Note::plain("164 tools used, 3 failed"),
        Note::plain("Context: 84k of 200k tokens (42%)"),
        Note::plain("1 sub-agent out"),
    ]
}

/// What a host says of a session that asks for a permission.
fn asked() -> Vec<Note> {
    let mut notes = vec![
        Note::urgent(
            "Wants to run: rm -rf target && cargo build --release --locked --workspace \
             --all-targets",
        ),
        Note::urgent("Open it to allow or refuse it in its terminal."),
    ];
    notes.extend(summary());
    notes
}

/// The keys as the app lists them.
fn keys() -> Vec<(String, String)> {
    [
        ("Tab, Down, Right", "the next lion"),
        ("Shift+Tab, Up, Left", "the previous lion"),
        ("N", "the next lion that needs you"),
        ("/ or G", "go to a lion by name, state or folder"),
        ("Enter", "open its terminal: answer it there"),
        ("I", "message it: typed at its prompt, or queued"),
        ("Q", "its queued messages, to take one back"),
        ("P", "message several lions of the pride"),
        ("X", "interrupt its agent in the middle of a turn"),
        ("R", "rename its session"),
        ("H", "send it home: asleep, to be woken later"),
        ("W", "wake a session that is at home"),
        ("A", "hatch a lion: a new agent session"),
        ("M", "its menu"),
        ("Page Up", "the feed, a page back"),
        ("Page Down", "the feed, a page on"),
        ("Home", "the start of the feed"),
        ("End", "the end of the feed"),
        ("Shift+Up", "the feed's entry before"),
        ("Shift+Down", "the feed's entry after"),
        ("E", "edit the room"),
        ("?", "these keys"),
        ("Escape", "let go of the entry, the lion, then the Den"),
    ]
    .into_iter()
    .map(|(key, what)| (key.to_owned(), what.to_owned()))
    .collect()
}

/// What a picture is of.
struct Shot<'a> {
    name: &'a str,
    size: (i32, i32),
    dark: bool,
    lions: usize,
    select: Option<u64>,
    notes: Vec<Note>,
    home: usize,
    keys: bool,
}

fn shot(gpu: &mut Renderer, out: &str, shot: &Shot<'_>, iso: bool) {
    let layout = leon_den::prefabs::prefab("office")
        .expect("a prefab")
        .layout;
    let palette = support::palette(shot.dark);
    let cubs = cast(shot.lions, shot.dark);
    let mut den = Den::new();
    den.set_layout(&layout, false, Duration::ZERO);
    den.update(&cubs, Duration::ZERO);
    den.select(shot.select);
    if let Some(id) = shot.select {
        den.set_notes(HashMap::from([(id, shot.notes.clone())]));
    }
    den.set_home(
        [
            "rowan",
            "Fix the flaky updater test",
            "hazel",
            "tansy",
            "brook",
        ]
        .iter()
        .take(shot.home)
        .enumerate()
        .map(|(index, name)| HomeEntry {
            id: 900 + index as u64,
            name: (*name).to_owned(),
            tint: support::tints(shot.dark)[index % 3],
        })
        .collect(),
    );
    den.hover_home(Some(900));
    den.set_keys(shot.keys.then(keys));
    let metrics = TextMetrics {
        char_w: 4. * UNIT as f32,
        line_h: 9. * UNIT as f32,
    };
    let (width, height) = shot.size;
    let view = Layout::compute_with_home(
        width,
        height,
        1.,
        metrics,
        layout.cols,
        layout.rows,
        den.order().len(),
        den.home().len(),
    );
    let at = Duration::from_secs(60);
    let frame = den.frame(at);
    let mut page = Bitmap::new(width, height);
    page.fill(0, 0, width, height, bytes(palette.ground));
    let quad = |page: &mut Bitmap, rect: Rect, color| {
        page.fill(rect.x, rect.y, rect.w, rect.h, bytes(color));
    };
    let scene = if iso {
        let theme = Theme::from_tokens(&support::tokens(shot.dark));
        let field = view.field;
        let picture = (field.w.max(16) as u32, field.h.max(16) as u32);
        let camera = Camera::fit(
            layout.cols,
            layout.rows,
            picture.0 as f32,
            picture.1 as f32,
            26.,
        );
        let glide = den.glide(at);
        let built = room::build(&den, &frame, &glide, at.as_secs_f32(), &theme, true, None);
        let mut pixels = gpu
            .draw(&built.mesh, &camera, &theme, picture)
            .expect("the room is drawn");
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let room_picture = Bitmap::from_rgba(picture.0 as i32, picture.1 as i32, pixels);
        let lettering = Lettering {
            char_w: 4. * UNIT as f32 * 0.84,
            high: 8 * UNIT,
            pad: 3 * UNIT,
        };
        let overlay = lay(
            &built.stands,
            &camera,
            (field.x, field.y),
            &palette,
            &lettering,
        );
        let boxes = |id: u64| box_of(&overlay, id);
        let scene = build_around(
            &den,
            &frame,
            &Layout { map: field, ..view },
            &palette,
            None,
            Some(&boxes),
        );
        for under in &scene.under {
            quad(&mut page, under.rect, under.color);
        }
        page.draw(&room_picture, scene.room.x, scene.room.y);
        for label in &overlay.labels {
            if let Some(leader) = label.leader {
                quad(&mut page, leader, label.line);
            }
            quad(&mut page, label.rect, label.line);
            let r = label.rect;
            page.fill(r.x + 1, r.y + 1, r.w - 2, r.h - 2, bytes(label.ground));
            write(
                &mut page,
                r.x + lettering.pad,
                r.y + (r.h - 5 * UNIT) / 2,
                &label.text,
                bytes(label.ink),
            );
        }
        scene
    } else {
        let scene = build(&den, &frame, &view, &palette, None);
        let mut wardrobe = Wardrobe::new();
        let picture = compose(&den, &frame, &palette, &mut wardrobe, None).scaled(view.unit);
        for under in &scene.under {
            quad(&mut page, under.rect, under.color);
        }
        page.draw(&picture, scene.room.x, scene.room.y);
        scene
    };
    for over in &scene.quads {
        quad(&mut page, over.rect, over.color);
    }
    for text in &scene.texts {
        write(
            &mut page,
            text.x,
            text.y + 2 * UNIT,
            &text.text,
            bytes(text.color),
        );
    }
    let name = format!(
        "{out}/{}-{}-{}.png",
        shot.name,
        if iso { "iso" } else { "pixel" },
        if shot.dark { "dark" } else { "light" }
    );
    std::fs::write(&name, page.to_png()).expect("the png is written");
    println!("{name}");
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("the directory to write into");
    let mut gpu = Renderer::start().expect("a GPU");
    let large = (1500, 940);
    let shots = [
        // The card of a lion that works, with the whole summary.
        Shot {
            name: "card-summary",
            size: large,
            dark: true,
            lions: 9,
            select: Some(1),
            notes: summary(),
            home: 0,
            keys: false,
        },
        Shot {
            name: "card-summary",
            size: large,
            dark: false,
            lions: 9,
            select: Some(1),
            notes: summary(),
            home: 0,
            keys: false,
        },
        // The card of a lion that asks for a permission.
        Shot {
            name: "card-permission",
            size: large,
            dark: true,
            lions: 9,
            select: Some(4),
            notes: asked(),
            home: 2,
            keys: false,
        },
        // The roster: who needs the user first, and who is at home.
        Shot {
            name: "roster-home",
            size: large,
            dark: true,
            lions: 9,
            select: None,
            notes: Vec::new(),
            home: 5,
            keys: false,
        },
        Shot {
            name: "roster-home",
            size: large,
            dark: false,
            lions: 5,
            select: None,
            notes: Vec::new(),
            home: 2,
            keys: false,
        },
        // The keys over the room.
        Shot {
            name: "keys",
            size: large,
            dark: true,
            lions: 9,
            select: Some(4),
            notes: asked(),
            home: 2,
            keys: true,
        },
        Shot {
            name: "keys-low",
            size: (1500, 420),
            dark: true,
            lions: 4,
            select: None,
            notes: Vec::new(),
            home: 0,
            keys: true,
        },
        // Small dens: the card says less and stays off its lion.
        Shot {
            name: "small-900x520",
            size: (900, 520),
            dark: true,
            lions: 6,
            select: Some(4),
            notes: asked(),
            home: 1,
            keys: false,
        },
        Shot {
            name: "small-640x360",
            size: (640, 360),
            dark: true,
            lions: 4,
            select: Some(4),
            notes: asked(),
            home: 0,
            keys: false,
        },
    ];
    for one in &shots {
        for iso in [true, false] {
            shot(&mut gpu, &out, one, iso);
        }
    }
}
