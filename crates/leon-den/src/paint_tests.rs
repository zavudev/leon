//! Tests of the painter: the picture of a frame.

use std::time::Duration;

use crate::atelier::{COAT, MANE, STYLES};
use crate::bitmap::Bitmap;
use crate::model::CubState;
use crate::paint::{
    coat_under, compose, dyed, mane, shaded, tag_name, Look, Wardrobe, COATS, SHADES,
};
use crate::palette::bytes;
use crate::pose::TILE;
use crate::sim::Den;
use crate::testing::{at, cub, little, palette};
use crate::world::Place;

fn den(cubs: &[crate::model::Cub]) -> Den {
    let mut den = Den::new();
    den.update(cubs, Duration::ZERO);
    den
}

fn picture(den: &Den, tick: u64, dark: bool) -> Bitmap {
    compose(
        den,
        &den.frame(at(tick)),
        &palette(dark),
        &mut Wardrobe::new(),
        None,
    )
}

fn count(picture: &Bitmap, color: [u8; 4]) -> usize {
    (0..picture.h)
        .flat_map(|y| (0..picture.w).map(move |x| (x, y)))
        .filter(|(x, y)| picture.get(*x, *y) == color)
        .count()
}

#[test]
fn the_picture_is_the_map_a_pixel_of_art_for_a_pixel() {
    let den = den(&[]);
    let room = picture(&den, 0, true);
    assert_eq!((room.w, room.h), (20 * TILE, 15 * TILE));
    // An empty den is its backdrop and its furniture: more than the
    // backdrop alone.
    assert_ne!(&room, den.world().backdrop());
}

#[test]
fn the_room_has_its_own_colours_whatever_the_theme() {
    let den = den(&[cub(1, "moss", CubState::Editing)]);
    let (dark, light) = (picture(&den, 400, true), picture(&den, 400, false));
    // Only the chrome differs: here, the plate under the lion's name.
    let differing = (0..dark.h)
        .flat_map(|y| (0..dark.w).map(move |x| (x, y)))
        .filter(|(x, y)| dark.get(*x, *y) != light.get(*x, *y))
        .count();
    assert!(differing > 0, "the name plate follows the theme");
    assert!(
        differing < 400,
        "{differing} pixels follow the theme: too many"
    );
}

#[test]
fn a_mane_is_dyed_in_the_tint_of_its_lion_and_no_key_colour_is_left() {
    let ramp = mane([0xd9, 0x77, 0x57]);
    assert_eq!(
        ramp[1],
        [0xd9, 0x77, 0x57],
        "a tint that is a mane is kept as it is"
    );
    let sheet = dyed(&crate::assets::art().lions[0][0], [0xd9, 0x77, 0x57], 0);
    for key in MANE {
        assert_eq!(count(&sheet, key), 0);
    }
    assert!(count(&sheet, [0xd9, 0x77, 0x57, 255]) > 500);

    let den = den(&[
        cub(1, "moss", CubState::WaitingForUser),
        little(9, 1, CubState::Editing),
    ]);
    let room = picture(&den, 400, true);
    for key in MANE {
        assert_eq!(count(&room, key), 0, "a key colour in the room");
    }
    // Each wears its own shade of the agent's colour.
    let worn: usize = [1, 9]
        .into_iter()
        .map(|seed| {
            let [r, g, b] = mane(shaded([0xd9, 0x77, 0x57], Look::of(seed).shade))[1];
            count(&room, [r, g, b, 255])
        })
        .sum();
    assert!(worn > 30, "{worn} pixels in the agent's colour");
}

#[test]
fn a_seed_is_one_look_and_the_seeds_are_many_looks() {
    let looks: Vec<Look> = (0..400).map(Look::of).collect();
    assert_eq!(looks, (0..400).map(Look::of).collect::<Vec<_>>());
    let bodies = crate::assets::art().lions.len();
    assert_eq!(
        Look::count(),
        bodies * STYLES.len() * COATS.len() * SHADES.len()
    );
    // Every body, head, coat and shade is somebody's.
    for body in 0..bodies {
        assert!(looks.iter().any(|look| look.body == body), "body {body}");
    }
    for style in 0..STYLES.len() {
        assert!(
            looks.iter().any(|look| look.style == style),
            "style {style}"
        );
    }
    for coat in 0..COATS.len() {
        assert!(looks.iter().any(|look| look.coat == coat), "coat {coat}");
    }
    for shade in 0..SHADES.len() {
        assert!(
            looks.iter().any(|look| look.shade == shade),
            "shade {shade}"
        );
    }
    // Two sessions are rarely the same lion: of 400, most looks are worn
    // once, and no head goes with one coat only.
    let mut distinct = looks.clone();
    distinct.sort_by_key(|look| (look.body, look.style, look.coat, look.shade));
    distinct.dedup();
    assert!(
        distinct.len() > 330,
        "{} looks for 400 seeds",
        distinct.len()
    );
    for style in 0..STYLES.len() {
        let mut coats: Vec<usize> = looks
            .iter()
            .filter(|look| look.style == style)
            .map(|look| look.coat)
            .collect();
        coats.sort_unstable();
        coats.dedup();
        assert_eq!(coats.len(), COATS.len(), "style {style}");
    }
    // A room of a dozen sessions of one agent is a dozen different sheets.
    let twelve: Vec<crate::model::Cub> = (1..=12)
        .map(|id| cub(id, "moss", CubState::WaitingForUser))
        .collect();
    let den = den(&twelve);
    let mut wardrobe = Wardrobe::new();
    compose(
        &den,
        &den.frame(at(400)),
        &palette(true),
        &mut wardrobe,
        None,
    );
    assert!(wardrobe.len() >= 10, "{} sheets for twelve", wardrobe.len());
}

#[test]
fn a_coat_dyes_the_fur_and_leaves_no_key_colour_but_the_golden_ones() {
    let art = crate::assets::art();
    let tint = [0xd9, 0x77, 0x57];
    // The golden coat is the sheet as it is drawn.
    assert_eq!(
        COATS[0].map(|[r, g, b]| [r, g, b, 255]),
        COAT,
        "the first coat is the workshop's"
    );
    for (index, coat) in COATS.iter().enumerate().skip(1) {
        for sheet in [&art.lions[0][0], &art.lions[2][1], &art.cub] {
            let worn = dyed(sheet, tint, index);
            for key in COAT {
                assert_eq!(count(&worn, key), 0, "coat {index} left a golden pixel");
            }
            let [r, g, b] = coat[1];
            assert!(count(&worn, [r, g, b, 255]) > 20, "coat {index} is worn");
            // Only the fur changed: the clothes and the mane are the same.
            let golden = dyed(sheet, tint, 0);
            for y in 0..sheet.h {
                for x in 0..sheet.w {
                    let was = sheet.get(x, y);
                    if !COAT.contains(&was) {
                        assert_eq!(worn.get(x, y), golden.get(x, y));
                    }
                }
            }
        }
        // A coat is fur: three steps of one colour, the light the lightest.
        assert!(lightness(coat[0]) > lightness(coat[1]) && lightness(coat[1]) > lightness(coat[2]));
        assert!(
            lightness(coat[3]) > lightness(coat[0]),
            "the muzzle is paler"
        );
    }
    // No two coats are the same fur.
    for (index, coat) in COATS.iter().enumerate() {
        for other in &COATS[index + 1..] {
            assert!(apart(coat[1], other[1]) > 30., "{coat:?} and {other:?}");
        }
    }
}

#[test]
fn a_shade_stays_in_the_family_of_its_agents_colour() {
    let (clay, blue) = ([0xd9, 0x77, 0x57], [0x3b, 0x82, 0xf6]);
    assert_eq!(
        shaded(clay, 0),
        clay,
        "the first shade is the colour itself"
    );
    let mut seen = Vec::new();
    for shade in 0..SHADES.len() {
        let (mine, other) = (mane(shaded(clay, shade))[1], mane(shaded(blue, shade))[1]);
        // Nearer to its own agent's colour than to another agent's, by far.
        assert!(apart(mine, clay) < 75., "shade {shade}: {mine:?}");
        assert!(apart(mine, clay) * 2. < apart(mine, blue), "shade {shade}");
        assert!(
            apart(other, blue) * 2. < apart(other, clay),
            "shade {shade}"
        );
        assert!(!seen.contains(&mine), "shade {shade} is another's");
        seen.push(mine);
    }
}

#[test]
fn the_wardrobe_dyes_a_sheet_once() {
    let den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Running),
        little(9, 1, CubState::Editing),
    ]);
    let mut wardrobe = Wardrobe::new();
    assert!(wardrobe.is_empty());
    let first = compose(
        &den,
        &den.frame(at(400)),
        &palette(true),
        &mut wardrobe,
        None,
    );
    let dyed = wardrobe.len();
    assert!(
        (1..=3).contains(&dyed),
        "{dyed} sheets for two lions and a cub"
    );
    let again = compose(
        &den,
        &den.frame(at(400)),
        &palette(true),
        &mut wardrobe,
        None,
    );
    assert_eq!(wardrobe.len(), dyed);
    assert_eq!(first, again, "the same frame is the same picture");
}

#[test]
fn a_computer_is_on_while_a_lion_types_at_it_and_off_when_it_leaves() {
    let mut den = den(&[cub(1, "moss", CubState::Editing)]);
    let seat = den.world().spots(Place::Desks)[0].tile;
    // The screen: over the seat, on the table.
    let screen = |room: &Bitmap| room.part(seat.x * TILE, (seat.y - 2) * TILE, TILE, TILE);
    let empty = picture(&self::den(&[]), 0, true);
    let typing = picture(&den, 400, true);
    assert_ne!(screen(&typing), screen(&empty), "the screen is lit");
    // And it flickers as the lion types.
    let frames: Vec<Bitmap> = (400..412)
        .map(|tick| screen(&picture(&den, tick, true)))
        .collect();
    assert!(frames.windows(2).any(|pair| pair[0] != pair[1]));
    den.update(&[cub(1, "moss", CubState::WaitingForUser)], at(400));
    assert_eq!(
        screen(&picture(&den, 800, true)),
        screen(&empty),
        "off again"
    );
}

#[test]
fn the_selected_lion_is_marked_in_the_accent_and_its_name_is_on_its_plate() {
    let mut den = den(&[cub(1, "moss", CubState::WaitingForUser)]);
    let accent = bytes(palette(true).accent);
    // The rug is yellow too: count against the same frame unselected.
    let plain = count(&picture(&den, 400, true), accent);
    den.select(Some(1));
    let marked = count(&picture(&den, 400, true), accent);
    assert!(marked > plain + 30, "{plain} then {marked} accent pixels");
    assert_eq!(tag_name("moss"), "MOSS");
}

#[test]
fn with_reduced_motion_the_picture_holds_still() {
    let mut den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Searching),
        cub(3, "ash", CubState::Running),
    ]);
    den.set_reduced_motion(true, Duration::ZERO);
    assert_eq!(picture(&den, 400, true), picture(&den, 437, true));
    den.set_reduced_motion(false, at(438));
    let moving: Vec<Bitmap> = (500..520).map(|tick| picture(&den, tick, true)).collect();
    assert!(moving.windows(2).any(|pair| pair[0] != pair[1]));
}

fn lightness(color: [u8; 3]) -> f32 {
    let max = f32::from(*color.iter().max().unwrap());
    let min = f32::from(*color.iter().min().unwrap());
    (max + min) / 510.
}

fn apart(a: [u8; 3], b: [u8; 3]) -> f32 {
    (0..3)
        .map(|channel| (f32::from(a[channel]) - f32::from(b[channel])).powi(2))
        .sum::<f32>()
        .sqrt()
}

#[test]
fn whatever_the_tint_a_mane_is_a_mane() {
    // The agents' colours of both themes, the extremes, and a blue.
    let tints: [[u8; 3]; 9] = [
        [0xd9, 0x77, 0x57],
        [0xb8, 0x58, 0x3a],
        [0xfa, 0xfa, 0xf9],
        [0x0c, 0x0a, 0x09],
        [0xf1, 0xec, 0xec],
        [0x21, 0x1e, 0x1e],
        [0xff, 0xff, 0xff],
        [0x00, 0x00, 0x00],
        [0x3b, 0x82, 0xf6],
    ];
    // The fur of the workshop: its light, its middle and its cream.
    let fur: [[u8; 3]; 3] = [[244, 200, 124], [226, 166, 88], [252, 236, 204]];
    for tint in tints {
        let [light, middle, dark] = mane(tint);
        // Three steps that show: the strands are drawn in them.
        assert!(
            lightness(light) - lightness(middle) >= 0.08,
            "{tint:?}: {light:?} {middle:?}"
        );
        assert!(
            lightness(middle) - lightness(dark) >= 0.10,
            "{tint:?}: {middle:?} {dark:?}"
        );
        // Neither a fleece nor black hair.
        let (low, high) = crate::paint::MANE_LIGHTNESS;
        assert!(
            (low - 0.01..=high + 0.01).contains(&lightness(middle)),
            "{tint:?}"
        );
        assert!(lightness(light) < 0.9 && lightness(dark) > 0.08, "{tint:?}");
        // And not the colour of the face it frames.
        for coat in fur {
            assert!(
                apart(middle, coat) >= 40.,
                "{tint:?}: {middle:?} is the fur {coat:?}"
            );
        }
    }
    // White, black and Claude's orange stay three different manes.
    let (white, black, clay) = (mane(tints[2])[1], mane(tints[3])[1], mane(tints[0])[1]);
    assert!(apart(white, black) > 120. && apart(white, clay) > 60. && apart(black, clay) > 100.);
    // A colourless tint is warmed: more red than blue.
    assert!(white[0] > white[2] && black[0] > black[2]);
}

#[test]
fn the_editor_marks_what_is_selected_and_shows_the_piece_in_hand_where_it_may_go() {
    use crate::editor::Editor;
    use crate::world::Tile;
    let den = den(&[]);
    let plain = picture(&den, 0, true);
    let mut editor = Editor::new(den.layout().clone());
    editor.pick("plant");
    editor.point(Some(Tile::new(6, 8)));
    let free = compose(
        &den,
        &den.frame(at(0)),
        &palette(true),
        &mut Wardrobe::new(),
        Some(&editor.marks()),
    );
    assert_ne!(free, plain, "the ghost is drawn");
    // Green where it may go: the tile is washed in the success colour.
    let tint = |room: &Bitmap, tile: Tile| room.get(tile.x * TILE + 1, tile.y * TILE + 14);
    let (was, ghost) = (tint(&plain, Tile::new(6, 8)), tint(&free, Tile::new(6, 8)));
    assert!(ghost[1] > was[1], "more green: {was:?} then {ghost:?}");
    // Red where it may not: on a desk.
    editor.point(Some(Tile::new(3, 5)));
    let taken = compose(
        &den,
        &den.frame(at(0)),
        &palette(true),
        &mut Wardrobe::new(),
        Some(&editor.marks()),
    );
    let (was, ghost) = (tint(&plain, Tile::new(3, 5)), tint(&taken, Tile::new(3, 5)));
    assert!(
        i32::from(ghost[0]) - i32::from(was[0]) > 2 * (i32::from(ghost[1]) - i32::from(was[1])),
        "more red: {was:?} then {ghost:?}"
    );
}

#[test]
fn every_floor_and_every_wall_paints_a_room_of_its_own() {
    use crate::assets::{FLOORS, WALLS};
    let mut seen = Vec::new();
    for floor in FLOORS {
        let mut room = crate::layout::DenLayout::empty("Bare", 9, 8);
        room.floor = floor.id.to_owned();
        let backdrop = crate::world::World::build(&room).backdrop().clone();
        assert!(
            !seen.contains(&backdrop),
            "{} looks like another floor",
            floor.id
        );
        seen.push(backdrop);
    }
    for wall in WALLS {
        let mut room = crate::layout::DenLayout::empty("Bare", 9, 8);
        room.wall = wall.id.to_owned();
        let backdrop = crate::world::World::build(&room).backdrop().clone();
        assert!(
            wall.id == "rock" || !seen.contains(&backdrop),
            "{}",
            wall.id
        );
        seen.push(backdrop);
    }
}

#[test]
fn a_host_gets_a_picture_of_a_room_of_a_piece_and_of_a_floor() {
    use crate::paint::{floor_tile, piece, thumbnail};
    for prefab in crate::prefabs::prefabs() {
        let picture = thumbnail(&prefab.layout, &palette(true));
        assert_eq!(
            (picture.w, picture.h),
            (prefab.layout.cols * TILE, prefab.layout.rows * TILE)
        );
        assert_eq!(picture, thumbnail(&prefab.layout, &palette(true)));
    }
    for entry in crate::catalogue::CATALOGUE {
        for turn in 0..entry.views.len() as u8 {
            let picture = piece(entry.id, turn).unwrap();
            assert_eq!(picture.w, entry.view(turn).w * TILE);
            assert!(picture.area() > 0, "{}", entry.id);
        }
    }
    // A chair seen from the left is the one seen from the right, mirrored.
    assert_eq!(
        piece("chair", 3).unwrap(),
        piece("chair", 1).unwrap().mirrored()
    );
    assert!(piece("throne", 0).is_none());
    for floor in crate::assets::FLOORS {
        let tile = floor_tile(floor.id).unwrap();
        assert_eq!((tile.w, tile.h), (TILE, TILE));
    }
    assert!(floor_tile("lava").is_none());
}

#[test]
fn no_name_and_no_room_makes_the_picture_or_the_narrator_panic() {
    use crate::model::{Event, Happening, ToolKind};
    let names = [
        "",
        " ",
        "a-name-much-longer-than-any-plate-could-ever-hold-on-a-tile",
        "完了しました。テストはすべて通ります。",
        "\u{1f981}\u{1f981}\nline two\ttab",
    ];
    for prefab in crate::prefabs::prefabs() {
        let mut den = crate::sim::Den::new();
        den.set_layout(&prefab.layout, false, at(0));
        let mut cubs = Vec::new();
        for (index, state) in CubState::ALL.iter().cycle().take(40).enumerate() {
            let name = names[index % names.len()];
            cubs.push(cub(index as u64 + 1, name, *state));
            if index % 5 == 0 {
                cubs.push(crate::testing::little(
                    1000 + index as u64,
                    index as u64 + 1,
                    *state,
                ));
            }
        }
        den.update(&cubs, at(0));
        for (index, name) in names.iter().enumerate() {
            for kind in [
                ToolKind::Edit,
                ToolKind::Run,
                ToolKind::Search,
                ToolKind::Other,
            ] {
                for detail in [None, Some((*name).to_owned()), Some(name.repeat(30))] {
                    den.happen(
                        &Happening::new(
                            index as u64 + 1,
                            *name,
                            Event::ToolStarted {
                                kind,
                                tool: (*name).to_owned(),
                                detail,
                            },
                        ),
                        at(1),
                    );
                }
            }
            den.happen(
                &Happening::new(
                    index as u64 + 1,
                    *name,
                    Event::SentOut {
                        little: name.repeat(9),
                    },
                ),
                at(1),
            );
        }
        for tick in [0, 1, 7, 40, 400, 4000] {
            let picture = picture(&den, tick, tick % 2 == 0);
            assert_eq!(picture.w, prefab.layout.cols * TILE);
        }
    }
}

#[test]
fn a_plate_says_the_first_words_of_a_name_that_mean_something_in_plain_letters() {
    use crate::paint::{plate_name, PLATE_CHARS, PLATE_CHARS_SHORT};
    let full = |name: &str| plate_name(name, PLATE_CHARS);
    // Whole words, as many as fit; the small words that join them are left out.
    assert_eq!(full("Chat móvil para el relay"), "CHAT MOVIL");
    assert_eq!(full("MCP para Fudo v3 y WhatsApp"), "MCP FUDO V3");
    assert_eq!(full("Otro PR"), "OTRO PR");
    assert_eq!(full("Leon integration"), "LEON");
    assert_eq!(full("fix the login bug"), "FIX LOGIN");
    // A first word that is too long is cut, and says so.
    assert_eq!(
        full("Lineamientos de voz y personalidad de marca"),
        "LINEAMIENTOS"
    );
    assert_eq!(
        full("Internationalization of the app"),
        "INTERNATION\u{2026}"
    );
    assert_eq!(full("  juniper-the-second "), "JUNIPER-THE\u{2026}");
    // Accents come off, the marks of Spanish go, ß and æ are written out.
    assert_eq!(full("¿Qué pasó, señor Ñandú?"), "QUE PASO");
    assert_eq!(full("Straße Ærø çà"), "STRASSE AERO");
    assert_eq!(full("ÁÉÍÓÚ Üñe"), "AEIOU UNE");
    // Nothing but small words is still a name; nothing writable is `?`.
    assert_eq!(full("de la"), "DE LA");
    assert_eq!(full("完了しました"), "?");
    assert_eq!(full(""), "?");
    assert_eq!(full("テスト api 修正"), "API");
    // Crowded, it is shorter still.
    let short = |name: &str| plate_name(name, PLATE_CHARS_SHORT);
    assert_eq!(short("Chat móvil para el relay"), "CHAT");
    assert_eq!(short("Lineamientos de voz"), "LINEA\u{2026}");
    assert_eq!(short("Otro PR"), "OTRO");
    for name in [
        "x",
        "Chat móvil para el relay",
        "Lineamientos",
        "a b c d e f g h",
    ] {
        for most in 0..20 {
            let plate = plate_name(name, most);
            assert!(
                plate.chars().count() <= most.max(2),
                "{name} in {most}: {plate}"
            );
            assert!(!plate.is_empty());
            // Every letter of it is one the little font draws.
            assert!(plate.chars().all(crate::glyphs::writes), "{plate}");
        }
    }
}

#[test]
fn the_little_font_draws_an_accented_letter_as_the_letter() {
    use crate::glyphs::{fold, glyph, plain, writes};
    for (accented, base) in [
        ('á', 'a'),
        ('Ñ', 'N'),
        ('ü', 'u'),
        ('Ç', 'C'),
        ('ø', 'o'),
        ('Ž', 'Z'),
    ] {
        assert_eq!(fold(accented), Some(base));
        assert_eq!(glyph(accented), glyph(base), "{accented}");
        assert!(writes(accented));
    }
    assert_eq!(fold('a'), None);
    assert_eq!(fold('完'), None);
    assert!(!writes('完') && !writes('\u{1f981}'));
    assert_eq!(plain("¡Olé, señor!"), "Ole, senor!");
    assert_ne!(
        glyph('\u{2026}'),
        glyph('完'),
        "the ellipsis has a glyph of its own"
    );
}

/// Whether two boxes of the picture cover a pixel in common.
fn boxes_meet(a: crate::paint::Area, b: crate::paint::Area) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

#[test]
fn no_two_plates_cover_each_other_or_a_face_and_none_leaves_the_picture() {
    use crate::paint::{bubble_shifts, head, plates};
    let names = [
        "Lineamientos de voz y personalidad de marca",
        "Chat móvil para el relay",
        "MCP para Fudo v3 y WhatsApp",
        "Otro PR",
        "WhatsApp embedded signup",
        "Leon integration",
        "Mirada HA",
        "x",
    ];
    let mixes: [&[CubState]; 5] = [
        &[CubState::Asleep],
        &[CubState::WaitingForUser],
        &[CubState::Reading],
        &[CubState::Idle],
        &CubState::ALL,
    ];
    let (mut named, mut lions) = (0usize, 0usize);
    for prefab in crate::prefabs::prefabs() {
        for states in mixes {
            for count in 1..=30usize {
                let mut den = crate::sim::Den::new();
                den.set_layout(&prefab.layout, false, at(0));
                let cubs: Vec<_> = (0..count)
                    .filter(|index| states[index % states.len()] != CubState::Gone)
                    .map(|index| {
                        cub(
                            index as u64 + 1,
                            names[index % names.len()],
                            states[index % states.len()],
                        )
                    })
                    .collect();
                den.update(&cubs, at(0));
                for tick in [4000, 4003, 4011] {
                    let frame = den.frame(at(tick));
                    let (w, h) = (prefab.layout.cols * TILE, prefab.layout.rows * TILE);
                    let placed = plates(&frame, w, h);
                    let what = format!("{} with {count} at {tick}", prefab.id);
                    for (index, plate) in placed.iter().enumerate() {
                        let (x, y, pw, ph) = plate.area;
                        assert!(
                            x >= 0 && y >= 0 && x + pw <= w && y + ph <= h,
                            "{what}: the plate of {} leaves the picture: {:?}",
                            plate.id,
                            plate.area
                        );
                        for other in &placed[index + 1..] {
                            assert!(
                                !boxes_meet(plate.area, other.area),
                                "{what}: {:?} over {:?}",
                                plate,
                                other
                            );
                        }
                        for actor in frame.actors.iter().filter(|a| a.id != plate.id) {
                            assert!(
                                !boxes_meet(plate.area, head(actor)),
                                "{what}: the plate of {} covers the face of {}",
                                plate.id,
                                actor.id
                            );
                        }
                    }
                    named += placed.len();
                    lions += frame.actors.len();
                    // A bubble moves at most half a tile from its lion, and
                    // only to stop covering a face.
                    for (id, shift) in bubble_shifts(&frame) {
                        assert!(
                            shift != 0 && shift.abs() <= TILE / 2,
                            "{what}: {id} by {shift}"
                        );
                    }
                }
            }
        }
    }
    // Hiding a plate is the last resort: nearly everybody is named.
    assert!(
        named * 100 >= lions * 85,
        "{named} plates for {lions} lions"
    );
}

#[test]
fn the_selected_lion_is_always_named_and_first() {
    use crate::paint::plates;
    let mut den = crate::sim::Den::new();
    let cubs: Vec<_> = (0..30)
        .map(|index| cub(index + 1, "Chat móvil para el relay", CubState::Asleep))
        .collect();
    den.set_layout(
        &crate::prefabs::prefab("nook").unwrap().layout,
        false,
        at(0),
    );
    den.update(&cubs, at(0));
    let frame = den.frame(at(4000));
    let before = plates(&frame, 10 * TILE, 9 * TILE);
    let hidden = frame
        .actors
        .iter()
        .find(|actor| !before.iter().any(|plate| plate.id == actor.id))
        .expect("a room this full cannot name everybody")
        .id;
    den.select(Some(hidden));
    let frame = den.frame(at(4000));
    let after = plates(&frame, 10 * TILE, 9 * TILE);
    assert_eq!((after[0].id, after[0].selected), (hidden, true));
    let (x, y, w, h) = after[0].area;
    assert!(x >= 0 && y >= 0 && x + w <= 10 * TILE && y + h <= 9 * TILE);
}

#[test]
fn a_mane_is_never_lost_on_its_coat() {
    // The agents' colours of both themes, the colourless ones too.
    let tints: [[u8; 3]; 7] = [
        [0xd9, 0x77, 0x57],
        [0xb8, 0x58, 0x3a],
        [0xfa, 0xfa, 0xf9],
        [0x0c, 0x0a, 0x09],
        [0xf1, 0xec, 0xec],
        [0x21, 0x1e, 0x1e],
        [0x3b, 0x82, 0xf6],
    ];
    for tint in tints {
        let mut worn = Vec::new();
        for shade in 0..SHADES.len() {
            let tint = shaded(tint, shade);
            for coat in 0..COATS.len() {
                let under = coat_under(coat, tint);
                let middle = mane(tint)[1];
                for fur in &COATS[under][..2] {
                    let apart: i32 = (0..3)
                        .map(|channel| (i32::from(fur[channel]) - i32::from(middle[channel])).abs())
                        .sum();
                    assert!(
                        apart >= 40,
                        "{tint:?} on coat {under}: {middle:?} on {fur:?}"
                    );
                }
                worn.push(under);
            }
        }
        // And every agent still has lions of several coats.
        worn.sort_unstable();
        worn.dedup();
        assert!(worn.len() >= 4, "{tint:?} wears only {worn:?}");
    }
    // A mane that shows keeps the coat its lion was given.
    assert_eq!(coat_under(3, [0x3b, 0x82, 0xf6]), 3);
}
