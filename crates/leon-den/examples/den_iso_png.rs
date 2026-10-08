//! Paints the Den in 2.5D into PNG files, with no window: to look at it.
//!
//! ```text
//! cargo run -p leon-den --features den3d --example den_iso_png -- OUT_DIR
//! ```
//!
//! It writes `iso-<prefab>-<dark|light>.png` for the built-in dens named in
//! `DEN_PREFABS` (comma separated; the office, the open plan and the
//! library when it is not set), each with a lion in every state, settled,
//! and `iso-office-dark-early.png`, the same den a moment after they came
//! in: eggs in the nest and lions on their way. The room is the picture the
//! view shows ([`leon_den::iso`]); the name plates and the bubbles are laid
//! out by the same code and written here in the Den's own little font,
//! since there is no text system.
//!
//! Environment: `DEN_ONE_AGENT=1` gives every lion the colour of one agent,
//! `DEN_SIZE=1400x900` sets the size of the picture, and
//! `DEN_SELECT=3` selects that lion.

#[allow(dead_code)]
#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use leon_den::bitmap::Bitmap;
use leon_den::glyphs;
use leon_den::iso::overlay::{lay, Lettering};
use leon_den::iso::{room, Camera, Renderer, Theme};
use leon_den::model::{Cub, CubState, Species};
use leon_den::palette::bytes;
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

const NAMES: [&str; 16] = [
    "whatsapp",
    "leon",
    "chat movil",
    "mcp",
    "otro pr",
    "engram",
    "omarchy",
    "opcion",
    "lineamientos",
    "mirada",
    "fudo v3",
    "explore",
    "review",
    "den",
    "release",
    "docs",
];

/// A lion in every state, and a little one for the one that delegates.
fn cast(dark: bool) -> Vec<Cub> {
    let tints = support::tints(dark);
    // A den of one agent's sessions: every mane is of the same colour.
    let one_agent = std::env::var_os("DEN_ONE_AGENT").is_some();
    let mut cubs: Vec<Cub> = CubState::ALL
        .iter()
        .filter(|state| **state != CubState::Gone)
        .enumerate()
        .map(|(index, state)| Cub {
            id: index as u64 + 1,
            name: NAMES[index % NAMES.len()].to_owned(),
            species: Species {
                tint: tints[if one_agent { 0 } else { index % 3 }],
                seed: index as u64 * 7 + 3,
            },
            state: *state,
            level: index as u32 * 3,
            detail: None,
            parent: None,
            mystery: *state == CubState::Mystery,
        })
        .collect();
    if let Some(parent) = cubs.iter().find(|cub| cub.state == CubState::Delegating) {
        cubs.push(Cub {
            id: 100,
            name: "explore".to_owned(),
            species: Species {
                tint: parent.species.tint,
                seed: 41,
            },
            state: CubState::Editing,
            level: 2,
            detail: None,
            parent: Some(parent.id),
            mystery: false,
        });
    }
    cubs
}

fn shot(
    gpu: &mut Renderer,
    out: &str,
    prefab: &str,
    dark: bool,
    at: Duration,
    tag: &str,
    size: (u32, u32),
) {
    let layout = leon_den::prefabs::prefab(prefab).expect("a prefab").layout;
    let mut den = Den::new();
    den.set_layout(&layout, false, Duration::ZERO);
    den.update(&cast(dark), Duration::ZERO);
    if let Some(select) = std::env::var("DEN_SELECT")
        .ok()
        .and_then(|n| n.parse().ok())
    {
        den.select(Some(select));
    }
    let frame = den.frame(at);
    let theme = Theme::from_tokens(&support::tokens(dark));
    let palette = support::palette(dark);
    let camera = Camera::fit(layout.cols, layout.rows, size.0 as f32, size.1 as f32, 28.);
    let began = Instant::now();
    let built = room::build(
        &den,
        &frame,
        &den.glide(at),
        at.as_secs_f32(),
        &theme,
        true,
        None,
    );
    let meshed = began.elapsed();
    let began = Instant::now();
    let pixels = gpu
        .draw(&built.mesh, &camera, &theme, size)
        .expect("the room is drawn");
    let drawn = began.elapsed();
    let mut rgba = pixels;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut picture = Bitmap::from_rgba(size.0 as i32, size.1 as i32, rgba);
    let unit = 2;
    let lettering = Lettering {
        char_w: 4. * unit as f32,
        high: 9 * unit,
        pad: 3 * unit,
    };
    let overlay = lay(&built.stands, &camera, (0, 0), &palette, &lettering);
    for label in &overlay.labels {
        let r = label.rect;
        picture.fill(r.x, r.y, r.w, r.h, bytes(label.line));
        picture.fill(r.x + 1, r.y + 1, r.w - 2, r.h - 2, bytes(label.ground));
        let text = glyphs::plain(&label.text);
        write(
            &mut picture,
            r.x + lettering.pad,
            r.y + 2 * unit,
            unit,
            &text,
            bytes(label.ink),
        );
    }
    let name = format!(
        "{out}/iso-{prefab}-{}{tag}.png",
        if dark { "dark" } else { "light" }
    );
    std::fs::write(&name, picture.to_png()).expect("the png is written");
    println!(
        "{name}: {} triangles, {} lions, mesh {:.2} ms, gpu {:.2} ms{}",
        built.mesh.triangles(),
        built.stands.len(),
        meshed.as_secs_f64() * 1000.,
        drawn.as_secs_f64() * 1000.,
        if built.plain.is_empty() {
            String::new()
        } else {
            format!(", plain boxes for {:?}", built.plain)
        }
    );
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("the directory to write into");
    let size = std::env::var("DEN_SIZE")
        .ok()
        .and_then(|size| {
            let (w, h) = size.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1400, 900));
    let prefabs =
        std::env::var("DEN_PREFABS").unwrap_or_else(|_| "office,open-plan,library".to_owned());
    let mut gpu = Renderer::start().expect("a GPU");
    println!("adapter: {}", gpu.adapter());
    let settled = Duration::from_secs(40);
    for prefab in prefabs.split(',') {
        for dark in [true, false] {
            shot(&mut gpu, &out, prefab, dark, settled, "", size);
        }
    }
    shot(
        &mut gpu,
        &out,
        "office",
        true,
        Duration::from_millis(2600),
        "-early",
        size,
    );
}
