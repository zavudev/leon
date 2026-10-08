//! Paints the whole view of the Den in 2.5D into PNG files, with no window:
//! the room, and over it everything the view lays out, to check it by eye.
//!
//! ```text
//! cargo run -p leon-den --features den3d --example den_view_png -- OUT_DIR
//! ```
//!
//! The room is the picture the view shows ([`leon_den::iso`]). The name
//! plates, the bubbles, the truth card, the roster and the feed are laid
//! out by the code the view uses ([`leon_den::iso::overlay`],
//! [`leon_den::scene`]) and written here in the Den's own little font,
//! since there is no text system without a window: the boxes are where the
//! view puts them, the letters are not the host's.
//!
//! It writes: the office with a crowd, dark and light; the office being
//! edited, a piece in hand where it may go and where it may not, a piece
//! selected; the empty den; a den of 24 lions; lions seen from behind; and
//! the pictures of the rooms and of every piece that a host lists
//! (`thumbs-*.png`). It prints what each picture took.

#[allow(dead_code)]
#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use leon_den::bitmap::Bitmap;
use leon_den::catalogue::CATALOGUE;
use leon_den::editor::Editor;
use leon_den::glyphs;
use leon_den::iso::overlay::{box_of, lay, Lettering};
use leon_den::iso::room::{self, Aim};
use leon_den::iso::{Camera, Renderer, Theme};
use leon_den::model::{Cub, CubState, Species};
use leon_den::palette::bytes;
use leon_den::scene::{build_around, Layout, Rect, TextMetrics};
use leon_den::world::Tile;
use leon_den::Den;

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

const NAMES: [&str; 12] = [
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
    "review",
];

fn lion(index: usize, state: CubState, dark: bool) -> Cub {
    Cub {
        id: index as u64 + 1,
        name: format!(
            "{}{}",
            NAMES[index % NAMES.len()],
            if index >= NAMES.len() { " 2" } else { "" }
        ),
        species: Species {
            tint: support::tints(dark)[index % 3],
            seed: index as u64 * 7 + 3,
        },
        state,
        level: index as u32 * 3,
        detail: Some("crates/app/src/ui/den.rs".to_owned()),
        parent: None,
        mystery: false,
    }
}

/// A crowd: `count` lions, a state each, in turn.
fn crowd(count: usize, dark: bool, states: &[CubState]) -> Vec<Cub> {
    (0..count)
        .map(|index| lion(index, states[index % states.len()], dark))
        .collect()
}

const EVERY: [CubState; 14] = [
    CubState::Editing,
    CubState::Reading,
    CubState::Searching,
    CubState::Running,
    CubState::Web,
    CubState::Delegating,
    CubState::Planning,
    CubState::UsingTool,
    CubState::Thinking,
    CubState::WaitingForUser,
    CubState::NeedsPermission,
    CubState::Idle,
    CubState::Asleep,
    CubState::Fainted,
];

/// Gives the tile the pointer finds over the middle of a tile, by an aim.
type TileAt<'a> = &'a dyn Fn(i32, i32, Aim) -> Tile;
/// What the editor does before a picture.
type Edit<'a> = &'a dyn Fn(&mut Editor, TileAt<'_>);

/// What a picture is of.
struct Shot<'a> {
    name: &'a str,
    prefab: &'a str,
    dark: bool,
    cubs: Vec<Cub>,
    select: Option<u64>,
    /// What the editor does before the picture, if the room is edited.
    edit: Option<Edit<'a>>,
    at: Duration,
}

fn shot(gpu: &mut Renderer, out: &str, size: (i32, i32), shot: Shot<'_>) {
    let layout = leon_den::prefabs::prefab(shot.prefab)
        .expect("a prefab")
        .layout;
    let mut den = Den::new();
    den.set_layout(&layout, false, Duration::ZERO);
    den.update(&shot.cubs, Duration::ZERO);
    den.select(shot.select);
    let theme = Theme::from_tokens(&support::tokens(shot.dark));
    let palette = support::palette(shot.dark);
    let metrics = TextMetrics {
        char_w: 4. * UNIT as f32,
        line_h: 9. * UNIT as f32,
    };
    let view = Layout::compute(
        size.0,
        size.1,
        1.,
        metrics,
        layout.cols,
        layout.rows,
        shot.cubs.len(),
    );
    let field = view.field;
    let picture = (field.w.max(16) as u32, field.h.max(16) as u32);
    let camera = Camera::fit(
        layout.cols,
        layout.rows,
        picture.0 as f32,
        picture.1 as f32,
        26.,
    );

    // The room as the editor leaves it, and its marks.
    let mut marks = None;
    if let Some(edit) = shot.edit {
        let mut editor = Editor::new(layout.clone());
        let plan = editor.layout().clone();
        // A tile by the pixel that shows the middle of it, as the pointer
        // would find it.
        let tile_at = |col: i32, row: i32, aim: Aim| {
            let (x, y) = camera.project([col as f32 + 0.5, 0., row as f32 + 0.5]);
            room::hit(&plan, &camera, x, y, aim).tile
        };
        edit(&mut editor, &tile_at);
        den.set_layout(editor.layout(), true, Duration::ZERO);
        marks = Some(editor.marks());
    }

    let frame = den.frame(shot.at);
    let glide = den.glide(shot.at);
    let began = Instant::now();
    let built = room::build(
        &den,
        &frame,
        &glide,
        shot.at.as_secs_f32(),
        &theme,
        true,
        marks.as_ref(),
    );
    let meshed = began.elapsed();
    let began = Instant::now();
    let mut pixels = gpu
        .draw(&built.mesh, &camera, &theme, picture)
        .expect("the room is drawn");
    let drawn = began.elapsed();
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

    let mut page = Bitmap::new(size.0, size.1);
    page.fill(0, 0, size.0, size.1, bytes(palette.ground));
    let quad = |page: &mut Bitmap, rect: Rect, color| {
        page.fill(rect.x, rect.y, rect.w, rect.h, bytes(color));
    };
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
    let name = format!("{out}/{}.png", shot.name);
    std::fs::write(&name, page.to_png()).expect("the png is written");
    println!(
        "{name}: {} triangles, {} lions, mesh {:.2} ms, gpu {:.2} ms",
        built.mesh.triangles(),
        built.stands.len(),
        meshed.as_secs_f64() * 1000.,
        drawn.as_secs_f64() * 1000.,
    );
}

/// The pictures a host lists: the rooms, and every piece, on one sheet.
fn thumbs(gpu: &mut Renderer, out: &str, dark: bool) {
    let theme = Theme::from_tokens(&support::tokens(dark));
    let palette = support::palette(dark);
    let (cell, room_w, room_h) = (112, 300, 180);
    let per_row = 8;
    let rows = CATALOGUE.len().div_ceil(per_row) as i32;
    let prefabs = leon_den::prefabs::prefabs();
    let mut page = Bitmap::new(
        (per_row as i32 * cell).max(prefabs.len() as i32 * room_w),
        room_h + rows * (cell + 16) + 8,
    );
    page.fill(0, 0, page.w, page.h, bytes(palette.ground));
    let picture = |pixels: Vec<u8>, w: i32, h: i32| {
        let mut pixels = pixels;
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Bitmap::from_rgba(w, h, pixels)
    };
    let mut spent = Duration::ZERO;
    let mut made = 0;
    for (index, prefab) in prefabs.iter().enumerate() {
        let began = Instant::now();
        let pixels = gpu
            .room_picture(
                &prefab.layout,
                &theme,
                (room_w as u32 - 8, room_h as u32 - 8),
            )
            .expect("a room");
        spent += began.elapsed();
        made += 1;
        page.draw(
            &picture(pixels, room_w - 8, room_h - 8),
            index as i32 * room_w + 4,
            4,
        );
    }
    println!(
        "rooms: {:.2} ms each",
        spent.as_secs_f64() * 1000. / f64::from(made)
    );
    let (mut spent, mut made) = (Duration::ZERO, 0);
    for (index, entry) in CATALOGUE.iter().enumerate() {
        let (col, row) = ((index % per_row) as i32, (index / per_row) as i32);
        let began = Instant::now();
        let pixels = gpu
            .piece_picture(entry.id, 0, &theme, (cell as u32 - 8, cell as u32 - 8))
            .expect("a piece");
        spent += began.elapsed();
        made += 1;
        let (x, y) = (col * cell + 4, room_h + row * (cell + 16) + 4);
        page.draw(&picture(pixels, cell - 8, cell - 8), x, y);
        write(
            &mut page,
            x,
            y + cell - 6,
            entry.id,
            bytes(palette.text_muted),
        );
    }
    println!(
        "pieces: {:.2} ms each",
        spent.as_secs_f64() * 1000. / f64::from(made)
    );
    let name = format!("{out}/thumbs-{}.png", if dark { "dark" } else { "light" });
    std::fs::write(&name, page.to_png()).expect("the png is written");
    println!("{name}");
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("the directory to write into");
    let mut gpu = Renderer::start().expect("a GPU");
    println!("adapter: {}", gpu.adapter());
    let size = (1500, 940);
    let settled = Duration::from_secs(60);

    for dark in [true, false] {
        shot(
            &mut gpu,
            &out,
            size,
            Shot {
                name: if dark {
                    "office-crowd-dark"
                } else {
                    "office-crowd-light"
                },
                prefab: "office",
                dark,
                cubs: crowd(14, dark, &EVERY),
                select: Some(10),
                edit: None,
                at: settled,
            },
        );
    }
    shot(
        &mut gpu,
        &out,
        size,
        Shot {
            name: "empty-dark",
            prefab: "office",
            dark: true,
            cubs: Vec::new(),
            select: None,
            edit: None,
            at: settled,
        },
    );
    for dark in [true, false] {
        shot(
            &mut gpu,
            &out,
            size,
            Shot {
                name: if dark {
                    "crowd-24-dark"
                } else {
                    "crowd-24-light"
                },
                prefab: "office",
                dark,
                cubs: crowd(24, dark, &EVERY),
                select: None,
                edit: None,
                at: settled,
            },
        );
    }
    // Everybody waits or idles: lions on their feet, many seen from behind.
    shot(
        &mut gpu,
        &out,
        size,
        Shot {
            name: "backs-dark",
            prefab: "office",
            dark: true,
            cubs: crowd(
                12,
                true,
                &[
                    CubState::Planning,
                    CubState::Reading,
                    CubState::Running,
                    CubState::Web,
                ],
            ),
            select: None,
            edit: None,
            at: settled,
        },
    );
    // The editor: a rack in hand on free floor, the same on a desk, a
    // painting on the wall, and a desk selected.
    let fits = |editor: &mut Editor, tile: TileAt<'_>| {
        editor.pick("rack");
        let free = (2..18)
            .flat_map(|col| (4..14).map(move |row| (col, row)))
            .map(|(col, row)| tile(col, row, Aim::Level(0.)))
            .find(|at| {
                let mut probe = editor.clone();
                probe.point(Some(*at));
                probe.marks().ghost.is_some_and(|(_, fits)| fits)
            })
            .expect("a free tile");
        editor.point(Some(free));
    };
    let refused = |editor: &mut Editor, tile: TileAt<'_>| {
        editor.pick("rack");
        let desk = editor
            .layout()
            .items
            .iter()
            .find(|piece| piece.id == "desk")
            .map(|piece| (piece.x + 1, piece.y + 1))
            .expect("a desk");
        editor.point(Some(tile(desk.0, desk.1, Aim::Level(0.))));
    };
    let chosen = |editor: &mut Editor, tile: TileAt<'_>| {
        let desk = editor
            .layout()
            .items
            .iter()
            .find(|piece| piece.id == "desk")
            .map(|piece| (piece.x + 1, piece.y + 1))
            .expect("a desk");
        let _ = editor.press(tile(desk.0, desk.1, Aim::Any));
        editor.release();
        editor.point(Some(tile(10, 12, Aim::Any)));
    };
    let hung = |editor: &mut Editor, _: TileAt<'_>| {
        editor.pick("lion_painting");
        let free = (1..19)
            .map(|col| Tile::new(col, 0))
            .find(|at| {
                let mut probe = editor.clone();
                probe.point(Some(*at));
                probe.marks().ghost.is_some_and(|(_, fits)| fits)
            })
            .expect("a free place on the wall");
        editor.point(Some(free));
    };
    for (name, edit) in [
        ("edit-fits-dark", &fits as Edit<'_>),
        ("edit-refused-dark", &refused),
        ("edit-selected-dark", &chosen),
        ("edit-wall-dark", &hung),
    ] {
        shot(
            &mut gpu,
            &out,
            size,
            Shot {
                name,
                prefab: "office",
                dark: true,
                cubs: crowd(5, true, &EVERY),
                select: None,
                edit: Some(edit),
                at: settled,
            },
        );
    }
    shot(
        &mut gpu,
        &out,
        size,
        Shot {
            name: "edit-fits-light",
            prefab: "office",
            dark: false,
            cubs: crowd(5, false, &EVERY),
            select: None,
            edit: Some(&fits),
            at: settled,
        },
    );
    for dark in [true, false] {
        thumbs(&mut gpu, &out, dark);
    }
}
