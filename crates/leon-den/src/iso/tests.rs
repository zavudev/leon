//! Tests of the room in 2.5D: everything but the GPU.

use std::time::Duration;

use crate::catalogue::CATALOGUE;
use crate::editor::Editor;
use crate::iso::camera::{Camera, WALL_HEIGHT};
use crate::iso::lion::{Build, Pose, Style, Traits, Wear, COATS};
use crate::iso::mesh::{Mesh, Part, FACE_FLOATS, LINE_FLOATS};
use crate::iso::overlay::{box_of, lay, pick, Lettering};
use crate::iso::pieces::{piece, Live, Slot, MODELLED};
use crate::iso::room::{build, furnish, hit, marks, slot, stands, Aim};
use crate::iso::theme::{Ruling, Theme};
use crate::layout::{DenLayout, Placed};
use crate::model::CubState;
use crate::pose::{Act, Bubble, Facing, Look};
use crate::prefabs::{default_layout, prefabs};
use crate::sim::Den;
use crate::testing::{cub, palette, tokens};
use crate::world::Tile;

fn theme(dark: bool) -> Theme {
    Theme::from_tokens(&tokens(dark))
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

const FIT: crate::iso::lion::Fit = crate::iso::lion::Fit {
    seat: 0.46,
    scale: 0.92,
};

const LETTERING: Lettering = Lettering {
    char_w: 7.,
    high: 16,
    pad: 5,
};

// ----------------------------------------------------------------- mesh

#[test]
fn a_box_is_twelve_triangles_and_twelve_hairlines_and_a_glow_has_no_normal() {
    let mut mesh = Mesh::new([1., 1., 1.], true);
    mesh.piece(Part::ROOT, [2., 1., 4.], [0.5; 3], [3., 0., 5.]);
    assert_eq!(mesh.triangles(), 12);
    assert_eq!(mesh.lines.len(), 12 * 2 * LINE_FLOATS);
    // It stands on the middle of its base: from 2 to 4 across, 0 to 1 up,
    // 3 to 7 deep.
    let corners: Vec<&[f32]> = mesh.faces.chunks(FACE_FLOATS).collect();
    let span = |axis: usize| {
        let values = corners.iter().map(|corner| corner[axis]);
        (
            values.clone().fold(f32::MAX, f32::min),
            values.fold(f32::MIN, f32::max),
        )
    };
    assert_eq!((span(0), span(1), span(2)), ((2., 4.), (0., 1.), (3., 7.)));
    assert!(corners.iter().all(|corner| close(
        corner[3] * corner[3] + corner[4] * corner[4] + corner[5] * corner[5],
        1.
    )));

    let mut plain = Mesh::new([1.; 3], false);
    plain.piece(Part::ROOT, [1.; 3], [0.5; 3], [0.; 3]);
    assert!(plain.lines.is_empty(), "no hairlines when they are off");
    plain.glow(Part::ROOT, [1.; 3], [0.5; 3], [0.; 3]);
    let glow = &plain.faces[12 * 3 * FACE_FLOATS..];
    assert!(glow
        .chunks(FACE_FLOATS)
        .all(|corner| corner[3..6] == [0.; 3]));
}

#[test]
fn a_part_turns_tips_and_grows_about_its_own_origin() {
    let quarter = std::f32::consts::FRAC_PI_2;
    // A quarter turn brings the front to where the right was.
    let turned = Part::at(1., 0., 1.).turned(quarter);
    let [x, y, z] = turned.point([0., 0., 1.]);
    assert!(close(x, 2.) && close(y, 0.) && close(z, 1.), "{x} {y} {z}");
    // Tipped forward a quarter, its top points to its front.
    let [x, y, z] = Part::ROOT.pitched(quarter).point([0., 1., 0.]);
    assert!(close(x, 0.) && close(y, 0.) && close(z, 1.), "{x} {y} {z}");
    // Twice the size, a point is twice as far, and a direction is still one
    // long.
    let big = Part::at(1., 1., 1.).scaled(2.);
    assert_eq!(big.point([1., 0., 0.]), [3., 1., 1.]);
    assert_eq!(big.direction([0., 3., 0.]), [0., 1., 0.]);
    // Moving goes along its own axes.
    assert_eq!(big.moved([0., 1., 0.]).point([0.; 3]), [1., 3., 1.]);
}

#[test]
fn every_face_of_a_ball_looks_out_whichever_way_its_part_is_turned() {
    for part in [
        Part::ROOT,
        Part::at(3., 1., 2.).turned(2.).pitched(-1.).scaled(0.5),
    ] {
        let mut mesh = Mesh::new([1.; 3], true);
        mesh.ball(part, [1., 0.8, 0.6], [0.5; 3], [0., 1., 0.], true);
        assert_eq!(mesh.triangles(), 80);
        let centre = part.point([0., 1., 0.]);
        for corner in mesh.faces.chunks(FACE_FLOATS) {
            let out = (0..3)
                .map(|axis| (corner[axis] - centre[axis]) * corner[3 + axis])
                .sum::<f32>();
            assert!(out > 0., "a face looks into the ball");
        }
    }
}

// --------------------------------------------------------------- camera

#[test]
fn the_camera_shows_the_whole_room_whatever_the_shape_of_the_picture() {
    for (cols, rows) in [(8, 8), (14, 11), (26, 15), (30, 19)] {
        for (w, h) in [(1400f32, 900f32), (400., 900.), (1600., 300.), (64., 64.)] {
            let margin = 20f32.min(w.min(h) / 8.);
            let camera = Camera::fit(cols, rows, w, h, margin);
            let (cols, rows) = (cols as f32, rows as f32);
            let (mut left, mut right, mut top, mut bottom) =
                (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for x in [0.8, cols - 0.9] {
                for z in [1.8, rows] {
                    for y in [-0.3, WALL_HEIGHT] {
                        let (px, py) = camera.project([x, y, z]);
                        left = left.min(px);
                        right = right.max(px);
                        top = top.min(py);
                        bottom = bottom.max(py);
                    }
                }
            }
            let slack = 0.5;
            assert!(
                left >= margin - slack && right <= w - margin + slack,
                "{cols}x{rows} in {w}x{h}"
            );
            assert!(
                top >= margin - slack && bottom <= h - margin + slack,
                "{cols}x{rows} in {w}x{h}"
            );
            // As large as it can be: it touches the margin one way at least.
            let tight = close(left, margin) && close(right, w - margin)
                || close(top, margin) && close(bottom, h - margin);
            assert!(tight || (left - margin).abs() < 1. || (top - margin).abs() < 1.);
        }
    }
}

#[test]
fn a_pixel_goes_back_to_the_point_of_the_floor_it_shows() {
    let camera = Camera::fit(14, 11, 1280., 800., 24.);
    for (x, z) in [(1., 2.), (7.5, 6.25), (13., 11.), (3.3, 9.9)] {
        for height in [0., 0.7] {
            let (px, py) = camera.project([x, height, z]);
            let (bx, bz) = camera.floor(px, py, height);
            assert!(
                close(bx, x) && close(bz, z),
                "{x},{z} came back as {bx},{bz}"
            );
        }
    }
}

#[test]
fn what_is_farther_into_the_room_is_higher_in_the_picture_and_less_near() {
    let camera = Camera::fit(14, 11, 1280., 800., 24.);
    let (_, far) = camera.project([3., 0., 3.]);
    let (_, near) = camera.project([10., 0., 9.]);
    assert!(far < near);
    assert!(camera.nearness([10., 0., 9.]) > camera.nearness([3., 0., 3.]));
    // Up in the room is up in the picture.
    assert!(camera.project([5., 2., 5.]).1 < camera.project([5., 0., 5.]).1);
    // Along the back wall goes right, out of it goes left.
    assert!(camera.project([9., 0., 5.]).0 > camera.project([5., 0., 5.]).0);
    assert!(camera.project([5., 0., 9.]).0 < camera.project([5., 0., 5.]).0);
}

#[test]
fn the_matrix_puts_a_point_where_the_projection_does() {
    let camera = Camera::fit(18, 13, 1000., 700., 30.);
    let matrix = camera.matrix();
    for point in [[1., 0., 2.], [9., 1.3, 7.], [17., 2.6, 13.]] {
        let clip = |row: usize| {
            matrix[row] * point[0]
                + matrix[4 + row] * point[1]
                + matrix[8 + row] * point[2]
                + matrix[12 + row]
        };
        let (px, py) = camera.project(point);
        assert!(close((clip(0) + 1.) / 2. * 1000., px));
        assert!(close((1. - clip(1)) / 2. * 700., py));
        assert!(
            (0. ..=1.).contains(&clip(2)),
            "depth {} is in the picture",
            clip(2)
        );
    }
    // The sun sees the whole room too.
    let sun = camera.sun_matrix();
    for point in [
        [0.8, 0., 1.8],
        [17.1, WALL_HEIGHT, 13.],
        [0.8, WALL_HEIGHT, 13.],
        [17.1, 0., 1.8],
    ] {
        for row in 0..3 {
            let value = sun[row] * point[0]
                + sun[4 + row] * point[1]
                + sun[8 + row] * point[2]
                + sun[12 + row];
            let (low, high) = if row == 2 { (0., 1.) } else { (-1., 1.) };
            assert!((low..=high).contains(&value), "the sun misses {point:?}");
        }
    }
}

// ---------------------------------------------------------------- theme

#[test]
fn the_room_takes_every_colour_from_the_theme_dark_and_light() {
    let (dark, light) = (theme(true), theme(false));
    assert!(dark.dark && !light.dark);
    let lum = |c: [f32; 3]| c[0] + c[1] + c[2];
    // Dark planes in the dark theme, light ones in the light, and furniture
    // that stands out from the floor in both.
    assert!(lum(dark.floor) < 0.5 && lum(light.floor) > 2.5);
    assert!(lum(dark.piece) > lum(dark.floor));
    assert!(lum(light.piece) < lum(light.floor));
    assert!(lum(dark.edge) > lum(dark.piece) && lum(light.edge) < lum(light.piece));
    // The accent is the theme's own, the same in both.
    assert_eq!(dark.accent, light.accent);
    assert_eq!(dark.accent, [1., 234. / 255., 0.]);
    // A screen is dark in both.
    assert!(lum(dark.ink_screen()) < 0.3 && lum(light.ink_screen()) < 0.3);
}

#[test]
fn a_carpet_is_a_tone_of_its_own_and_a_floor_is_ruled_as_its_style_says() {
    for t in [theme(true), theme(false)] {
        let room = crate::assets::floor("wood").unwrap();
        assert_eq!(t.floor_of(room, room), t.floor);
        let carpets: Vec<[f32; 3]> = ["red", "green", "blue", "plum"]
            .into_iter()
            .map(|id| t.floor_of(crate::assets::floor(id).unwrap(), room))
            .collect();
        for (index, carpet) in carpets.iter().enumerate() {
            assert_ne!(*carpet, t.floor, "a carpet reads as a zone");
            assert!(!carpets[..index].contains(carpet), "two carpets are apart");
        }
    }
    let ruling = |id: &str| Theme::ruling(crate::assets::floor(id).unwrap());
    assert_eq!(ruling("wood"), Ruling::Planks);
    assert_eq!(ruling("slate"), Ruling::Tiles);
    assert_eq!(ruling("stone"), Ruling::Slabs);
    assert_eq!(ruling("checker"), Ruling::Checker);
    assert_eq!(ruling("red"), Ruling::Plain);
}

// --------------------------------------------------------------- pieces

#[test]
fn every_piece_of_the_catalogue_has_a_drawing_of_its_own() {
    let t = theme(true);
    for entry in CATALOGUE {
        assert!(MODELLED.contains(&entry.id), "{} is not modelled", entry.id);
        for (turn, view) in entry.views.iter().enumerate() {
            let mut layout = DenLayout::empty("one", 12, 12);
            layout
                .items
                .push(Placed::new(entry.id, 4, 4).turned(turn as u8));
            let slot = slot(&layout, 0).expect("a piece of the catalogue has a slot");
            let mut mesh = Mesh::new(t.edge, true);
            assert!(
                piece(&mut mesh, &t, entry.id, &slot, &Live::default()),
                "{}",
                entry.id
            );
            assert!(
                mesh.triangles() >= 12,
                "{} turned {turn} is drawn",
                entry.id
            );
            assert!(mesh.faces.iter().all(|value| value.is_finite()));
            // It stays about its own tiles: nothing of it is a tile away.
            for corner in mesh.faces.chunks(FACE_FLOATS) {
                assert!(corner[0] > slot.x - 1.1 && corner[0] < slot.x + view.w as f32 + 1.1);
                assert!(
                    corner[1] > -0.01 && corner[1] < WALL_HEIGHT + 0.6,
                    "{}",
                    entry.id
                );
            }
        }
    }
    assert_eq!(MODELLED.len(), CATALOGUE.len());
}

#[test]
fn a_piece_with_no_drawing_is_a_plain_box_of_its_tiles() {
    let t = theme(false);
    let mut mesh = Mesh::new(t.edge, true);
    let slot = Slot {
        x: 3.,
        z: 4.,
        w: 2.,
        d: 1.,
        y: 0.,
        facing: None,
        row: 0,
        rows: 0,
    };
    assert!(!piece(
        &mut mesh,
        &t,
        "a piece of tomorrow",
        &slot,
        &Live::default()
    ));
    assert_eq!(mesh.triangles(), 12);
}

#[test]
fn a_piece_is_where_the_layout_puts_it_on_the_floor_a_table_or_the_wall() {
    let layout = default_layout();
    let index = |id: &str| layout.items.iter().position(|item| item.id == id).unwrap();
    // A desk stands on the tiles under its picture's lower row.
    let desk = slot(&layout, index("desk")).unwrap();
    let placed = &layout.items[index("desk")];
    assert_eq!(
        (desk.x, desk.z, desk.w, desk.d, desk.y),
        (placed.x as f32, placed.y as f32 + 1., 3., 1., 0.)
    );
    // A computer stands on the desk under it, and looks at its seat.
    let pc = slot(&layout, index("pc")).unwrap();
    assert_eq!(pc.y, crate::iso::pieces::TABLE);
    assert_eq!(pc.facing, Some(Facing::Down));
    // What hangs on the wall is on the wall, in its rows.
    let shelf = slot(&layout, index("double_bookshelf")).unwrap();
    assert_eq!((shelf.z + shelf.d, shelf.row, shelf.rows), (2., 0, 2));
    // An id the catalogue does not know has no place.
    let mut odd = layout.clone();
    odd.items.push(Placed::new("nothing", 3, 3));
    assert!(slot(&odd, odd.items.len() - 1).is_none());
}

#[test]
fn a_screen_is_lit_in_the_colour_it_is_given_and_a_rack_runs_on_the_tick() {
    let t = theme(true);
    let slot = Slot {
        x: 4.,
        z: 4.,
        w: 1.,
        d: 1.,
        y: 0.7,
        facing: Some(Facing::Down),
        row: 0,
        rows: 0,
    };
    let draw = |id: &str, live: Live| {
        let mut mesh = Mesh::new(t.edge, true);
        piece(&mut mesh, &t, id, &slot, &live);
        mesh
    };
    let off = draw("pc", Live::default());
    let on = draw(
        "pc",
        Live {
            screen: Some(t.success),
            ..Live::default()
        },
    );
    assert!(on.triangles() > off.triangles());
    let has = |mesh: &Mesh, color: [f32; 3]| {
        mesh.faces
            .chunks(FACE_FLOATS)
            .any(|corner| corner[6..9] == color)
    };
    assert!(has(&on, t.success) && !has(&off, t.success));
    let at = |tick| {
        draw(
            "rack",
            Live {
                tick,
                running: true,
                ..Live::default()
            },
        )
    };
    assert_ne!(at(0), at(3), "the lights of a rack that runs move");
    assert_eq!(
        draw("rack", Live::default()),
        draw(
            "rack",
            Live {
                tick: 9,
                ..Live::default()
            }
        )
    );
}

// ------------------------------------------------------------------ room

#[test]
fn every_built_in_den_is_drawn_whole_with_no_plain_box() {
    for prefab in prefabs() {
        for dark in [true, false] {
            let t = theme(dark);
            let mut mesh = Mesh::new(t.edge, true);
            let plain = furnish(&mut mesh, &t, &prefab.layout, |_| Live::default());
            assert!(plain.is_empty(), "{}: {plain:?}", prefab.id);
            assert!(mesh.triangles() > 500, "{}", prefab.id);
            assert!(mesh
                .faces
                .iter()
                .chain(&mesh.lines)
                .all(|value| value.is_finite()));
            // Nothing is outside the room.
            let (cols, rows) = (prefab.layout.cols as f32, prefab.layout.rows as f32);
            for corner in mesh.faces.chunks(FACE_FLOATS) {
                assert!(corner[0] > 0.5 && corner[0] < cols - 0.5, "{}", prefab.id);
                assert!(corner[2] > 1.5 && corner[2] < rows + 0.5, "{}", prefab.id);
            }
        }
    }
}

#[test]
fn the_default_office_has_its_floor_its_walls_and_a_drawing_for_each_piece() {
    let layout = default_layout();
    let t = theme(true);
    let mut empty = Mesh::new(t.edge, true);
    let bare = DenLayout {
        items: Vec::new(),
        ..layout.clone()
    };
    furnish(&mut empty, &t, &bare, |_| Live::default());
    // The floor, a carpet, and three walls: five boxes.
    assert_eq!(empty.triangles(), 12 * 5);
    assert!(!empty.lines.is_empty(), "the floor is ruled");
    let mut full = Mesh::new(t.edge, true);
    furnish(&mut full, &t, &layout, |_| Live::default());
    assert!(full.triangles() > empty.triangles() + layout.items.len() * 12);
    // Without hairlines the room has no line at all.
    let mut plain = Mesh::new(t.edge, false);
    furnish(&mut plain, &t, &layout, |_| Live::default());
    assert!(plain.lines.is_empty());
    assert_eq!(plain.triangles(), full.triangles());
}

fn den_of(states: &[CubState]) -> Den {
    let mut den = Den::new();
    let cubs: Vec<_> = states
        .iter()
        .enumerate()
        .map(|(index, state)| cub(index as u64 + 1, &format!("lion {index}"), *state))
        .collect();
    den.update(&cubs, Duration::ZERO);
    den
}

#[test]
fn every_lion_of_a_frame_is_drawn_where_the_den_has_it() {
    let den = den_of(&CubState::ALL);
    let now = Duration::from_secs(60);
    let frame = den.frame(now);
    let t = theme(true);
    let empty = build(&Den::new(), &Den::new().frame(now), &[], 0., &t, true, None);
    let built = build(&den, &frame, &den.glide(now), 60., &t, true, None);
    assert_eq!(built.stands.len(), frame.actors.len());
    assert!(built.plain.is_empty());
    assert!(built.mesh.triangles() > empty.mesh.triangles() + frame.actors.len() * 100);
    for (actor, stand) in frame.actors.iter().zip(&built.stands) {
        assert_eq!(actor.id, stand.id);
        // Settled, it stands in the middle of its tile; those that wait
        // for the user stand in their line at the entrance, near it.
        let tile = [actor.tile.x as f32 + 0.5, 0., actor.tile.y as f32 + 0.5];
        if matches!(actor.look.act, Act::Wait | Act::Stare) {
            let far = (stand.feet[0] - tile[0]).hypot(stand.feet[2] - tile[2]);
            assert!(far < 2.5, "{} is {far} tiles from its place", stand.name);
        } else {
            assert_eq!(stand.feet, tile);
        }
        assert!(stand.height > 0.4 && stand.height < 2.2);
    }
    // Where they stand is the same without drawing them.
    assert_eq!(stands(&den, &frame, &den.glide(now), 60.), built.stands);
}

#[test]
fn a_lion_at_work_lights_the_screen_it_sits_at_and_a_command_runs_the_racks() {
    let t = theme(true);
    let now = Duration::from_secs(60);
    let picture = |state| {
        let den = den_of(&[state]);
        build(&den, &den.frame(now), &den.glide(now), 60., &t, true, None).mesh
    };
    let has = |mesh: &Mesh, color: [f32; 3], many: usize| {
        mesh.faces
            .chunks(FACE_FLOATS)
            .filter(|corner| corner[6..9] == color)
            .count()
            >= many
    };
    // The lines of a terminal, in the colour of what it does.
    assert!(has(&picture(CubState::Editing), t.success, 5 * 36));
    assert!(has(&picture(CubState::Thinking), t.info, 5 * 36));
    assert!(!has(&picture(CubState::Asleep), t.success, 5 * 36));
    // More lights are on while a command runs than the one of a rack at
    // rest.
    let lights = |mesh: &Mesh| {
        mesh.faces
            .chunks(FACE_FLOATS)
            .filter(|corner| corner[6..9] == t.success)
            .count()
    };
    assert!(lights(&picture(CubState::Running)) > lights(&picture(CubState::Asleep)));
}

#[test]
fn a_walk_glides_between_two_tiles_and_only_then_is_anybody_walking() {
    let mut den = Den::new();
    den.update(&[cub(1, "moss", CubState::Editing)], Duration::ZERO);
    // On its way from the door, between two steps of the pixel art.
    let mut between = None;
    for millis in (150..6000).step_by(50) {
        let now = Duration::from_millis(millis);
        if !den.walking(now) {
            continue;
        }
        let (_, x, y) = den.glide(now)[0];
        if x.fract() != 0. || y.fract() != 0. {
            between = Some((now, x, y));
            break;
        }
    }
    let (now, x, y) = between.expect("it walks to its desk");
    let actor = &den.frame(now).actors[0];
    let (ax, ay) = (actor.x as f32 / 16., actor.y as f32 / 16.);
    assert!(
        (x - ax).abs() <= 0.25 && (y - ay).abs() <= 0.25,
        "the glide is the same walk"
    );
    // Settled, nobody walks and the glide is the tile.
    let later = Duration::from_secs(60);
    assert!(!den.walking(later));
    let (_, x, y) = den.glide(later)[0];
    let tile = den.frame(later).actors[0].tile;
    assert_eq!((x, y), (tile.x as f32, tile.y as f32));
    // With reduced motion nobody walks at all.
    let mut still = Den::new();
    still.set_reduced_motion(true, Duration::ZERO);
    still.update(&[cub(1, "moss", CubState::Editing)], Duration::ZERO);
    assert!(!still.walking(Duration::from_millis(300)));
}

// ------------------------------------------------------------------ lion

#[test]
fn a_seed_always_gives_the_same_lion_and_the_lions_differ() {
    let all: Vec<Traits> = (0..400u64).map(Traits::of).collect();
    assert_eq!(all, (0..400u64).map(Traits::of).collect::<Vec<_>>());
    for coat in 0..COATS.len() {
        assert!(all.iter().any(|traits| traits.coat == coat));
    }
    for style in Style::ALL {
        assert!(all.iter().any(|traits| traits.style == style));
    }
    assert!(all.iter().any(|traits| traits.glasses) && all.iter().any(|traits| !traits.glasses));
    for shirt in 0..4 {
        assert!(all.iter().any(|traits| traits.shirt == shirt));
    }
    for shade in 0..crate::paint::SHADES.len() {
        assert!(all.iter().any(|traits| traits.shade == shade));
    }
    for build in Build::ALL {
        assert!(all.iter().any(|traits| traits.build == build));
    }
    for wear in Wear::ALL {
        assert!(all.iter().any(|traits| traits.wear == wear));
    }
    let distinct: std::collections::HashSet<Traits> = all.iter().copied().collect();
    assert!(distinct.len() > 300, "only {} lions in 400", distinct.len());
}

#[test]
fn the_sessions_of_one_agent_are_not_one_lion_many_times() {
    // A den is a dozen or two sessions, often all of one agent: the mane's
    // colour is then the same for all, and the rest must tell them apart.
    // Seeds as the app makes them are hashes; any run of them will do.
    for start in [0u64, 1_000, 0xcbf2_9ce4_8422_2325] {
        let den: Vec<Traits> = (0..14u64)
            .map(|index| Traits::of(start.wrapping_add(index.wrapping_mul(0x9e37_79b9))))
            .collect();
        let distinct: std::collections::HashSet<Traits> = den.iter().copied().collect();
        assert_eq!(distinct.len(), den.len(), "two lions alike from {start}");
        // And not by one small thing only: seen from behind, where neither
        // glasses nor a tie show, at least ten of the fourteen still differ.
        let behind: std::collections::HashSet<_> = den
            .iter()
            .map(|traits| {
                let wear = match traits.wear {
                    Wear::Tie => Wear::Nothing,
                    wear => wear,
                };
                (traits.style, traits.shade, traits.build, traits.shirt, wear)
            })
            .collect();
        assert!(behind.len() >= 10, "only {} from behind", behind.len());
        // Several shades of the one colour are in the room.
        let shades: std::collections::HashSet<usize> =
            den.iter().map(|traits| traits.shade).collect();
        assert!(shades.len() >= 4, "only {} shades", shades.len());
    }
}

#[test]
fn every_build_and_everything_worn_is_drawn_and_changes_the_lion() {
    let theme = Theme::from_tokens(&crate::testing::tokens(true));
    let drawn = |traits: &Traits| {
        let mut mesh = Mesh::default();
        crate::iso::lion::lion(
            &mut mesh,
            &theme,
            Part::at(0., 0., 0.),
            traits,
            [0.85, 0.47, 0.34],
            &Pose::of(&look(Act::Stand, false, 0), 0.),
            crate::iso::lion::Fit {
                seat: 0.46,
                scale: crate::iso::lion::SCALE,
            },
        );
        mesh.faces
    };
    let plain = Traits {
        coat: 0,
        style: Style::Mane,
        glasses: false,
        shirt: 0,
        shade: 0,
        build: Build::Even,
        wear: Wear::Nothing,
    };
    let base = drawn(&plain);
    for build in [Build::Slim, Build::Broad] {
        assert_ne!(drawn(&Traits { build, ..plain }), base, "{build:?}");
    }
    for wear in [Wear::Cap, Wear::Headset, Wear::Bow, Wear::Tie] {
        let with = drawn(&Traits { wear, ..plain });
        assert!(with.len() > base.len(), "{wear:?} adds something");
    }
    for shade in 1..crate::paint::SHADES.len() {
        assert_ne!(drawn(&Traits { shade, ..plain }), base, "shade {shade}");
    }
}

const ACTS: [Act; 18] = [
    Act::Egg,
    Act::Hatch,
    Act::Walk,
    Act::Stand,
    Act::Type,
    Act::Think,
    Act::Wonder,
    Act::Browse,
    Act::Rummage,
    Act::Operate,
    Act::Peer,
    Act::Plan,
    Act::Guard,
    Act::Wait,
    Act::Stare,
    Act::Lounge,
    Act::Sleep,
    Act::Faint,
];

fn look(act: Act, seat: bool, beat: u64) -> Look {
    Look {
        act,
        facing: Facing::Down,
        little: false,
        seat,
        beat,
        bubble: None,
    }
}

#[test]
fn every_act_has_a_pose_and_those_that_move_move_on_their_beat() {
    let still = Pose::of(&look(Act::Stand, false, 0), 0.);
    for act in ACTS {
        let pose = Pose::of(&look(act, true, 0), 1.);
        // It sits only where the act is done sitting.
        assert_eq!(pose.seated, act.seated(), "{act:?}");
        assert!(
            !Pose::of(&look(act, false, 0), 1.).seated,
            "{act:?} with no seat stands"
        );
        let moves = Pose::of(&look(act, true, 1), 1.) != pose;
        let named = !matches!(
            act,
            Act::Egg | Act::Hatch | Act::Stand | Act::Walk | Act::Faint
        );
        assert_eq!(moves, named, "{act:?} on its beat");
        if named || act == Act::Faint || act == Act::Walk {
            assert_ne!(pose, still, "{act:?} is a pose of its own");
        }
    }
    // A walk goes on the time it is given, not on its beat.
    assert_ne!(
        Pose::of(&look(Act::Walk, false, 0), 0.5),
        Pose::of(&look(Act::Walk, false, 0), 2.)
    );
    assert!(Pose::of(&look(Act::Faint, false, 0), 0.).lying);
    // No two of the working acts look the same.
    let working = [
        Act::Type,
        Act::Think,
        Act::Wonder,
        Act::Browse,
        Act::Rummage,
        Act::Operate,
        Act::Peer,
        Act::Plan,
        Act::Guard,
        Act::Wait,
        Act::Stare,
        Act::Lounge,
        Act::Sleep,
    ];
    for (index, act) in working.iter().enumerate() {
        for other in &working[..index] {
            assert_ne!(
                Pose::of(&look(*act, true, 0), 0.),
                Pose::of(&look(*other, true, 0), 0.),
                "{act:?} and {other:?}"
            );
        }
    }
}

#[test]
fn every_lion_and_every_egg_is_drawn_in_every_pose() {
    let t = theme(true);
    let mane = [0.85, 0.47, 0.34];
    for act in ACTS {
        for seat in [false, true] {
            for beat in 0..4 {
                let mut mesh = Mesh::new(t.edge, true);
                let feet = Part::at(4.5, 0., 4.5);
                match act {
                    Act::Egg | Act::Hatch => {
                        crate::iso::lion::egg(&mut mesh, &t, feet, mane, beat, act == Act::Hatch)
                    }
                    _ => {
                        let pose = Pose::of(&look(act, seat, beat), beat as f32);
                        for seed in [0, 5, 9, 31] {
                            crate::iso::lion::lion(
                                &mut mesh,
                                &t,
                                feet,
                                &Traits::of(seed),
                                mane,
                                &pose,
                                FIT,
                            );
                        }
                    }
                }
                assert!(mesh.triangles() > 60, "{act:?}");
                assert!(mesh.faces.iter().all(|value| value.is_finite()), "{act:?}");
                // A lion has no hairlines: it is not furniture.
                assert!(mesh.lines.is_empty());
                // It is about its tile and over the floor.
                for corner in mesh.faces.chunks(FACE_FLOATS) {
                    assert!(
                        (corner[0] - 4.5).abs() < 1.6 && (corner[2] - 4.5).abs() < 1.6,
                        "{act:?}"
                    );
                    assert!(
                        corner[1] > -0.05 && corner[1] < 2.4,
                        "{act:?} at {}",
                        corner[1]
                    );
                }
            }
        }
    }
}

#[test]
fn the_mane_is_the_colour_of_the_agent_and_a_lioness_wears_it_as_a_scarf() {
    let t = theme(false);
    let mane = [0.1, 0.9, 0.3];
    let pose = Pose::of(&look(Act::Stand, false, 0), 0.);
    for seed in 0..60 {
        let traits = Traits::of(seed);
        let mut mesh = Mesh::new(t.edge, true);
        crate::iso::lion::lion(&mut mesh, &t, Part::ROOT, &traits, mane, &pose, FIT);
        let agent = mesh
            .faces
            .chunks(FACE_FLOATS)
            .filter(|corner| corner[6..9] == crate::iso::lion::shade_of(mane, traits.shade))
            .count();
        assert!(
            agent >= 36,
            "seed {seed} ({:?}) shows its agent",
            traits.style
        );
    }
}

// --------------------------------------------------------------- overlay

#[test]
fn every_lion_has_a_plate_under_it_a_box_around_it_and_its_bubble_over_it() {
    let den = den_of(&[
        CubState::Editing,
        CubState::WaitingForUser,
        CubState::Asleep,
        CubState::Fainted,
    ]);
    let now = Duration::from_secs(60);
    let frame = den.frame(now);
    let stands = stands(&den, &frame, &den.glide(now), 60.);
    let camera = Camera::fit(14, 11, 1280., 800., 24.);
    let palette = palette(true);
    let overlay = lay(&stands, &camera, (100, 50), &palette, &LETTERING);
    assert_eq!(overlay.boxes.len(), 4);
    for stand in &stands {
        let rect = box_of(&overlay, stand.id).expect("a box");
        let (x, y) = camera.project(stand.feet);
        let (x, y) = (100 + x.round() as i32, 50 + y.round() as i32);
        assert!(
            x >= rect.x && x <= rect.x + rect.w,
            "the feet are in the box"
        );
        assert!(rect.h > 20 && rect.y < y);
        let plate = overlay
            .labels
            .iter()
            .find(|label| label.id == stand.id && !label.bubble)
            .expect("a plate");
        assert!(plate.rect.y >= y, "the plate is under the feet");
        assert!(plate.text.starts_with("LION"), "{}", plate.text);
        match stand.bubble {
            Some(_) => {
                let bubble = overlay
                    .labels
                    .iter()
                    .find(|label| label.id == stand.id && label.bubble)
                    .expect("a bubble");
                assert!(
                    bubble.rect.y + bubble.rect.h <= rect.y + 12,
                    "the bubble is over the head"
                );
            }
            None => assert!(!overlay
                .labels
                .iter()
                .any(|label| label.id == stand.id && label.bubble)),
        }
    }
    // Who waits has the accent, who fainted the error colour.
    let of = |kind: Bubble| {
        let id = stands
            .iter()
            .find(|stand| stand.bubble == Some(kind))
            .unwrap()
            .id;
        overlay
            .labels
            .iter()
            .find(|label| label.id == id && label.bubble)
            .unwrap()
            .clone()
    };
    assert_eq!(
        (of(Bubble::Bang).ground, of(Bubble::Bang).text.as_str()),
        (palette.accent, "!")
    );
    assert_eq!(of(Bubble::Cross).ink, palette.error);
}

#[test]
fn the_pointer_finds_the_lion_nearest_the_eye_and_a_plate_is_its_lion() {
    let den = den_of(&[
        CubState::WaitingForUser,
        CubState::WaitingForUser,
        CubState::Editing,
    ]);
    let now = Duration::from_secs(60);
    let frame = den.frame(now);
    let stands = stands(&den, &frame, &den.glide(now), 60.);
    let camera = Camera::fit(14, 11, 1280., 800., 24.);
    let overlay = lay(&stands, &camera, (0, 0), &palette(true), &LETTERING);
    // In the middle of its box, each lion is found.
    for pick_box in &overlay.boxes {
        let rect = pick_box.rect;
        let found = pick(&overlay, rect.x + rect.w / 2, rect.y + rect.h / 2).expect("a lion");
        let nearest = overlay
            .boxes
            .iter()
            .filter(|other| {
                other
                    .rect
                    .contains(rect.x + rect.w / 2, rect.y + rect.h / 2)
            })
            .max_by(|a, b| a.near.total_cmp(&b.near))
            .unwrap();
        assert_eq!(found, nearest.id);
    }
    // On a plate, the plate's lion.
    for label in overlay.labels.iter().filter(|label| !label.bubble) {
        let hit = pick(&overlay, label.rect.x + 1, label.rect.y + 1).expect("a lion");
        let top = overlay
            .labels
            .iter()
            .rev()
            .find(|other| !other.bubble && other.rect.contains(label.rect.x + 1, label.rect.y + 1))
            .unwrap();
        assert_eq!(hit, top.id);
    }
    // Far from everybody, nobody.
    assert_eq!(pick(&overlay, 2, 2), None);
}

#[test]
fn the_plates_of_lions_side_by_side_do_not_cover_each_other_and_the_selected_one_is_marked() {
    let mut den = den_of(&[CubState::WaitingForUser; 3]);
    den.select(Some(2));
    let now = Duration::from_secs(60);
    let frame = den.frame(now);
    let stands = stands(&den, &frame, &den.glide(now), 60.);
    let camera = Camera::fit(14, 11, 900., 600., 24.);
    let palette = palette(false);
    let overlay = lay(&stands, &camera, (0, 0), &palette, &LETTERING);
    let plates: Vec<_> = overlay
        .labels
        .iter()
        .filter(|label| !label.bubble)
        .collect();
    assert_eq!(plates.len(), 3);
    for (index, plate) in plates.iter().enumerate() {
        for other in &plates[..index] {
            let (a, b) = (plate.rect, other.rect);
            let apart =
                a.x >= b.x + b.w || b.x >= a.x + a.w || a.y >= b.y + b.h || b.y >= a.y + a.h;
            assert!(apart, "{} is on {}", plate.text, other.text);
        }
    }
    let selected = plates.iter().find(|plate| plate.id == 2).unwrap();
    assert_eq!(
        (selected.ground, selected.ink),
        (palette.accent, palette.on_accent)
    );
    // It is painted last of the plates: over the others.
    assert_eq!(plates.last().unwrap().id, 2);
    // An egg has no plate.
    let mut young = Den::new();
    let mut little = cub(9, "explore", CubState::Editing);
    little.parent = Some(1);
    young.update(
        &[cub(1, "moss", CubState::Delegating), little],
        Duration::ZERO,
    );
    let early = Duration::from_millis(500);
    let stands = crate::iso::room::stands(&young, &young.frame(early), &young.glide(early), 0.5);
    let overlay = lay(&stands, &camera, (0, 0), &palette, &LETTERING);
    let egg = stands
        .iter()
        .find(|stand| stand.egg)
        .expect("the little one is an egg");
    assert!(!overlay
        .labels
        .iter()
        .any(|label| label.id == egg.id && !label.bubble));
}

// ---------------------------------------------------------------- editor

/// The pixel that shows a point of the room.
fn pixel(camera: &Camera, point: [f32; 3]) -> (f32, f32) {
    camera.project(point)
}

#[test]
fn the_pointer_finds_the_tile_of_floor_under_it() {
    let layout = default_layout();
    let camera = Camera::fit(layout.cols, layout.rows, 1200., 800., 24.);
    for (col, row) in [(1, 2), (7, 9), (layout.cols - 2, layout.rows - 1)] {
        let (x, y) = pixel(&camera, [col as f32 + 0.5, 0., row as f32 + 0.5]);
        let found = hit(&layout, &camera, x, y, Aim::Level(0.));
        assert_eq!(found.tile, Tile::new(col, row));
        assert_eq!((found.height, found.item), (0., None));
    }
    // A pixel off the room is a tile off the plan: the editor refuses it.
    let off = hit(&layout, &camera, 2., 798., Aim::Level(0.));
    assert!(!layout.is_floor(off.tile));
}

#[test]
fn a_piece_is_taken_by_any_part_of_it_that_shows_and_the_nearest_wins() {
    let layout = default_layout();
    let camera = Camera::fit(layout.cols, layout.rows, 1200., 800., 24.);
    let (index, rack) = layout
        .items
        .iter()
        .enumerate()
        .find(|(_, piece)| piece.id == "rack")
        .expect("the office has a rack");
    let place = slot(&layout, index).unwrap();
    // The top of the rack is over floor that is far behind it in the
    // picture: the pointer finds the rack, and a tile it stands on.
    let top = [place.x + place.w / 2., 2.0, place.z + place.d / 2.];
    let (x, y) = pixel(&camera, top);
    let found = hit(&layout, &camera, x, y, Aim::Any);
    assert_eq!(found.item, Some(index));
    assert!(rack.footprint().contains(&found.tile));
    assert!(found.height > 1.5);
    // The floor that pixel would show, with an aim at the floor, is another
    // tile: what a piece in hand for the floor follows.
    let floor = hit(&layout, &camera, x, y, Aim::Level(0.));
    assert_ne!(floor.tile, found.tile);
    assert_eq!(floor.item, None);
    // An editor given that tile takes the rack.
    let mut editor = Editor::new(layout.clone());
    editor.press(found.tile).unwrap();
    assert_eq!(editor.selected(), Some(index));
    assert_eq!(editor.dragging(), Some(index));
    assert_eq!(editor.marks().chosen, Some(index));
    // Held at the height it was taken by, it follows the pointer by the
    // tiles under that height: a pointer that has not moved moves nothing.
    let held = hit(&layout, &camera, x, y, Aim::Level(found.height));
    assert_eq!(held.tile, found.tile);
}

#[test]
fn the_back_wall_is_found_by_its_two_rows_and_a_table_by_its_top() {
    let layout = default_layout();
    let camera = Camera::fit(layout.cols, layout.rows, 1200., 800., 24.);
    for (height, row) in [(2.2, 0), (0.9, 1)] {
        let (x, y) = pixel(&camera, [6.5, height, 2.0]);
        let found = hit(&layout, &camera, x, y, Aim::Wall);
        assert_eq!(found.tile, Tile::new(6, row), "at {height}");
        assert!(close(found.height, height));
    }
    // Off the wall, a piece that hangs follows the floor: it is refused
    // there, and shown so.
    let (x, y) = pixel(&camera, [6.5, 0., 9.5]);
    assert_eq!(hit(&layout, &camera, x, y, Aim::Wall).tile, Tile::new(6, 9));
    // What stands on a table is put by the table's top.
    let (index, desk) = layout
        .items
        .iter()
        .enumerate()
        .find(|(_, piece)| piece.id == "desk")
        .expect("the office has a desk");
    let place = slot(&layout, index).unwrap();
    let top = [place.x + 0.5, crate::iso::pieces::TABLE, place.z + 0.5];
    let (x, y) = pixel(&camera, top);
    let found = hit(&layout, &camera, x, y, Aim::Surface);
    assert_eq!(found.item, Some(index));
    assert!(desk.footprint().contains(&found.tile));
    assert_eq!(found.tile, Tile::new(place.x as i32, place.z as i32));
}

#[test]
fn the_marks_of_the_editor_are_drawn_in_the_room() {
    let layout = default_layout();
    let t = theme(true);
    let drawn = |edit: &dyn Fn(&mut Editor)| {
        let mut editor = Editor::new(layout.clone());
        edit(&mut editor);
        let mut mesh = Mesh::new(t.edge, true);
        marks(&mut mesh, &t, editor.layout(), &editor.marks());
        mesh
    };
    let has = |mesh: &Mesh, color: [f32; 3]| {
        mesh.faces
            .chunks(FACE_FLOATS)
            .any(|corner| corner[6..9] == color)
            || mesh.lines.chunks(LINE_FLOATS).any(|end| end[3..6] == color)
    };
    // Nothing in hand and the pointer nowhere: nothing is drawn.
    assert_eq!(drawn(&|_| {}).triangles(), 0);
    // A piece in hand where it may go: its tiles and itself, in green.
    let free = (2..layout.cols - 2)
        .flat_map(|col| (3..layout.rows).map(move |row| Tile::new(col, row)))
        .find(|tile| {
            let mut editor = Editor::new(layout.clone());
            editor.pick("plant");
            editor.point(Some(*tile));
            editor.marks().ghost.is_some_and(|(_, fits)| fits)
        })
        .expect("a free tile");
    let fits = drawn(&|editor| {
        editor.pick("plant");
        editor.point(Some(free));
    });
    assert!(fits.triangles() > 24);
    assert!(has(&fits, t.success) && !has(&fits, t.error));
    // On a desk it may not: in red.
    let desk = layout
        .items
        .iter()
        .find(|piece| piece.id == "desk")
        .unwrap();
    let refused = drawn(&|editor| {
        editor.pick("rack");
        editor.point(Some(Tile::new(desk.x + 1, desk.y + 1)));
    });
    assert!(has(&refused, t.error) && !has(&refused, t.success));
    // A selected piece has its box drawn in the accent.
    let chosen = drawn(&|editor| {
        editor.press(Tile::new(desk.x + 1, desk.y + 1)).unwrap();
        editor.release();
    });
    assert!(has(&chosen, t.accent));
    assert!(chosen.lines.len() >= 12 * 2 * LINE_FLOATS);
    // A carpet in hand marks the tile it would take; an empty hand the
    // tile under it.
    let carpet = drawn(&|editor| {
        editor.pick_carpet(Some(crate::assets::FLOORS[1].id));
        editor.point(Some(free));
    });
    assert!(carpet.triangles() >= 12);
    let cursor = drawn(&|editor| editor.point(Some(free)));
    assert!(cursor.triangles() >= 12);
    // The room is drawn with them when it is edited, and without otherwise.
    let den = den_of(&[CubState::Editing]);
    let now = Duration::from_secs(60);
    let mut editor = Editor::new(den.layout().clone());
    editor.pick("plant");
    editor.point(Some(free));
    let with = build(
        &den,
        &den.frame(now),
        &[],
        60.,
        &t,
        true,
        Some(&editor.marks()),
    );
    let without = build(&den, &den.frame(now), &[], 60., &t, true, None);
    assert!(with.mesh.triangles() > without.mesh.triangles());
}

#[test]
fn a_washed_theme_keeps_the_floor_and_takes_the_colour() {
    let t = theme(true);
    let washed = t.washed(t.error, 0.5);
    assert_eq!((washed.floor, washed.background), (t.floor, t.background));
    assert_eq!(washed.edge, t.error);
    assert_ne!(washed.piece, t.piece);
}

// ------------------------------------------------------------- pictures

#[test]
fn a_camera_about_a_box_shows_all_of_it() {
    let (low, high) = ([2., 0., 3.], [3., 2.1, 5.]);
    for (w, h) in [(96., 96.), (240., 80.), (60., 200.)] {
        let camera = Camera::around(low, high, w, h, 6.);
        for x in [low[0], high[0]] {
            for y in [low[1], high[1]] {
                for z in [low[2], high[2]] {
                    let (px, py) = camera.project([x, y, z]);
                    assert!(px >= 5.9 && px <= w - 5.9, "{px} of {w}");
                    assert!(py >= 5.9 && py <= h - 5.9, "{py} of {h}");
                }
            }
        }
    }
}

#[test]
fn every_piece_of_the_catalogue_can_be_drawn_alone_inside_its_box() {
    let t = theme(false);
    for entry in CATALOGUE {
        for turn in 0..entry.views.len() as u8 {
            let (mesh, low, high) = crate::iso::pieces::alone(&t, entry.id, turn)
                .unwrap_or_else(|| panic!("{} is a piece", entry.id));
            assert!(mesh.triangles() >= 12, "{} is drawn", entry.id);
            assert!(
                (0..3).all(|axis| high[axis] > low[axis]),
                "{} fills a box",
                entry.id
            );
        }
    }
    assert!(crate::iso::pieces::alone(&t, "no_such_piece", 0).is_none());
}

#[test]
fn a_flat_shape_is_a_fan_of_triangles_that_gives_its_own_light() {
    let mut mesh = Mesh::new([1.; 3], true);
    let square = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    mesh.shape(Part::at(4., 0., 4.), &square, 0.5, [0.2, 0.4, 0.6]);
    assert_eq!(mesh.triangles(), 2);
    assert!(mesh.lines.is_empty());
    for corner in mesh.faces.chunks(FACE_FLOATS) {
        assert!((4. ..=5.).contains(&corner[0]) && close(corner[1], 0.5));
        assert_eq!(corner[3..6], [0., 0., 0.]);
    }
}

// --------------------------------------------------------------- crowds

#[test]
fn lions_that_share_a_spot_stand_apart_and_a_seated_one_stays() {
    // More lions than the floor of the smallest room has spots for the
    // rug: the last of them share tiles.
    let mut den = Den::new();
    den.set_layout(&crate::prefabs::small_office(), false, Duration::ZERO);
    let cubs: Vec<_> = (0..40)
        .map(|index| {
            cub(
                index + 1,
                &format!("lion {index}"),
                CubState::WaitingForUser,
            )
        })
        .collect();
    den.update(&cubs, Duration::ZERO);
    let now = Duration::from_secs(120);
    let frame = den.frame(now);
    let placed = stands(&den, &frame, &den.glide(now), 120.);
    let mut tiles = std::collections::HashMap::new();
    for actor in &frame.actors {
        *tiles.entry(actor.tile).or_insert(0) += 1;
    }
    // Whoever shares a tile is not on the same point as another.
    for (index, stand) in placed.iter().enumerate() {
        for other in &placed[..index] {
            assert!(
                stand.feet != other.feet,
                "{} stands on {}",
                stand.name,
                other.name
            );
        }
    }
    // And whoever has a tile of its own is in the middle of it, but for
    // those at the entrance, who stand in their line.
    for (actor, stand) in frame.actors.iter().zip(&placed) {
        let in_line = matches!(actor.look.act, Act::Wait | Act::Stare) && !actor.look.seat;
        if tiles[&actor.tile] == 1 && !in_line {
            assert_eq!(
                stand.feet,
                [actor.tile.x as f32 + 0.5, 0., actor.tile.y as f32 + 0.5]
            );
        }
    }
}

#[test]
fn those_that_wait_for_the_user_stand_in_a_line_each_wholly_in_view() {
    for (prefab, waiting) in [
        ("office", 2),
        ("office", 3),
        ("open-plan", 3),
        ("library", 3),
    ] {
        let layout = crate::prefabs::prefab(prefab).unwrap().layout;
        let mut den = Den::new();
        den.set_layout(&layout, false, Duration::ZERO);
        let mut cubs: Vec<_> = (0..waiting)
            .map(|index| {
                cub(
                    index + 1,
                    &format!("lion {index}"),
                    CubState::WaitingForUser,
                )
            })
            .collect();
        cubs.push(cub(90, "busy", CubState::Editing));
        den.update(&cubs, Duration::ZERO);
        let now = Duration::from_secs(120);
        let frame = den.frame(now);
        let placed = stands(&den, &frame, &den.glide(now), 120.);
        let camera = Camera::fit(layout.cols, layout.rows, 1400., 900., 28.);
        let line: Vec<_> = frame
            .actors
            .iter()
            .zip(&placed)
            .filter(|(actor, _)| matches!(actor.look.act, Act::Wait | Act::Stare))
            .map(|(_, stand)| stand)
            .collect();
        assert_eq!(
            line.len(),
            waiting as usize,
            "{prefab}: all at the entrance"
        );
        for (index, stand) in line.iter().enumerate() {
            let (x, y) = camera.project(stand.feet);
            // Inside the room.
            assert!(
                stand.feet[0] > 0.3 && stand.feet[2] > 0.3,
                "{prefab}: {} is in the wall",
                stand.name
            );
            for other in &line[..index] {
                let (ox, oy) = camera.project(other.feet);
                let (_, top) = camera.project([stand.feet[0], stand.height, stand.feet[2]]);
                // What one tile across the eye's line is in the picture.
                let c = std::f32::consts::FRAC_1_SQRT_2;
                let unit = camera.project([c, 0., -c]).0 - camera.project([0., 0., 0.]).0;
                let wide = unit * (stand.half + other.half);
                // Side by side for the eye: no body over another.
                assert!(
                    (x - ox).abs() >= wide * 0.9 || (y - oy).abs() >= (y - top) * 0.9,
                    "{prefab}: {} stands over {}",
                    stand.name,
                    other.name
                );
            }
        }
        // Whoever works is where it was.
        let (actor, stand) = frame
            .actors
            .iter()
            .zip(&placed)
            .find(|(actor, _)| actor.id == 90)
            .unwrap();
        assert_eq!(
            stand.feet,
            [actor.tile.x as f32 + 0.5, 0., actor.tile.y as f32 + 0.5]
        );
    }
}

#[test]
fn a_plate_keeps_off_the_faces_of_other_lions_and_is_tied_to_its_own() {
    // A crowd before the rug: plates go down to free places, each with a
    // line to its lion, and none goes farther than its reach.
    let den = den_of(&[CubState::WaitingForUser; 9]);
    let now = Duration::from_secs(60);
    let frame = den.frame(now);
    let stands = stands(&den, &frame, &den.glide(now), 60.);
    let layout = den.layout();
    let camera = Camera::fit(layout.cols, layout.rows, 700., 460., 20.);
    let overlay = lay(&stands, &camera, (0, 0), &palette(true), &LETTERING);
    let plates: Vec<_> = overlay
        .labels
        .iter()
        .filter(|label| !label.bubble)
        .collect();
    assert_eq!(plates.len(), 9);
    let step = LETTERING.high + 2;
    for plate in &plates {
        let stand = stands.iter().find(|stand| stand.id == plate.id).unwrap();
        let (x, y) = camera.project(stand.feet);
        let home = y.round() as i32 + LETTERING.high / 3;
        // Under its own lion, at most so many lines down.
        assert_eq!(plate.rect.x + plate.rect.w / 2, x.round() as i32);
        assert!(plate.rect.y >= home && plate.rect.y <= home + step * 6);
        match plate.leader {
            Some(leader) => {
                assert!(plate.rect.y > home);
                assert_eq!(leader.y + leader.h, plate.rect.y);
                assert_eq!(leader.w, 1);
            }
            None => assert_eq!(plate.rect.y, home),
        }
    }
    assert!(overlay
        .labels
        .iter()
        .all(|label| !label.bubble || label.leader.is_none()));
}

// -------------------------------------------------------------- renderer

#[test]
fn a_build_with_no_gpu_in_it_cannot_start_the_renderer_and_says_so() {
    if crate::iso::Renderer::BUILT {
        return;
    }
    let why = crate::iso::Renderer::start().err().expect("no renderer");
    assert!(why.contains("no 2.5D renderer"));
}

/// The GPU itself: only where there is one. Run with
/// `cargo test -p leon-den --features den3d -- --ignored`.
#[cfg(feature = "den3d")]
#[test]
#[ignore = "needs a graphics adapter"]
fn the_gpu_draws_the_office_in_the_colours_of_the_theme() {
    let mut renderer = crate::iso::Renderer::start().expect("an adapter");
    let den = den_of(&[CubState::Editing, CubState::WaitingForUser]);
    let now = Duration::from_secs(60);
    for dark in [true, false] {
        let t = theme(dark);
        let built = build(&den, &den.frame(now), &den.glide(now), 60., &t, true, None);
        let camera = Camera::fit(14, 11, 640., 400., 16.);
        let pixels = renderer
            .draw(&built.mesh, &camera, &t, (640, 400))
            .expect("a picture");
        assert_eq!(pixels.len(), 640 * 400 * 4);
        // The corner is the page, blue first; the rug's accent is somewhere.
        let byte = |value: f32| (value * 255.).round() as u8;
        assert_eq!(
            &pixels[..3],
            &[
                byte(t.background[2]),
                byte(t.background[1]),
                byte(t.background[0])
            ]
        );
        assert!(
            pixels.chunks(4).any(|pixel| pixel[..3] == [0, 234, 255]),
            "the rug is in the picture"
        );
    }
    // A size it cannot draw is an error, not a panic.
    let t = theme(true);
    let camera = Camera::fit(14, 11, 0., 0., 0.);
    assert!(renderer
        .draw(&Mesh::new(t.edge, true), &camera, &t, (0, 0))
        .is_err());
}
