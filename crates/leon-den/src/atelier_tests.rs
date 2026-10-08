//! Tests of the workshop: the committed sheets are the ones it makes, and
//! what it makes is a lion.

use crate::atelier::{
    cub_sheet, everything, lion_file, lion_sheet, COAT, CUB_FRAME, CUB_FRAMES, FRAMES,
    FRAME_ASLEEP, FRAME_H, FRAME_OUT, FRAME_STARE, FRAME_W, MANE, SOURCES, STYLES,
};
use crate::bitmap::Bitmap;

fn source(index: usize) -> Bitmap {
    let bytes: &[u8] = match index {
        0 => include_bytes!("../assets/source/characters/char_0.png"),
        2 => include_bytes!("../assets/source/characters/char_2.png"),
        3 => include_bytes!("../assets/source/characters/char_3.png"),
        4 => include_bytes!("../assets/source/characters/char_4.png"),
        _ => include_bytes!("../assets/source/characters/char_5.png"),
    };
    Bitmap::from_png(bytes)
}

fn committed(name: &str) -> Bitmap {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    Bitmap::from_png(&std::fs::read(&path).unwrap_or_else(|_| panic!("{name} is committed")))
}

fn frame(sheet: &Bitmap, row: i32, index: i32) -> Bitmap {
    sheet.part(index * FRAME_W, row * FRAME_H, FRAME_W, FRAME_H)
}

fn is_mane(pixel: [u8; 4]) -> bool {
    MANE.contains(&pixel)
}

#[test]
fn the_committed_sheets_are_what_the_workshop_makes() {
    let files = everything(source);
    assert_eq!(files.len(), SOURCES.len() * STYLES.len() + 1 + 7);
    for (name, made) in files {
        assert!(
            committed(&name) == made,
            "{name} is stale: run `cargo run -p leon-den --example make_lions`"
        );
    }
}

#[test]
fn the_app_draws_every_sheet_the_workshop_makes_and_no_other_is_committed() {
    let art = crate::assets::art();
    assert_eq!(art.lions.len(), SOURCES.len());
    for (body, styles) in art.lions.iter().enumerate() {
        assert_eq!(styles.len(), STYLES.len());
        for (style, sheet) in styles.iter().enumerate() {
            assert!(
                *sheet == committed(&lion_file(body, style)),
                "body {body} in style {style} is another file"
            );
        }
    }
    let lions = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/lions");
    let files = std::fs::read_dir(lions)
        .expect("the lions are committed")
        .count();
    assert_eq!(files, SOURCES.len() * STYLES.len() + 1, "a stale sheet");
}

#[test]
fn the_key_colours_of_the_mane_and_of_the_coat_are_in_no_source_sheet() {
    for index in SOURCES {
        let sheet = source(index);
        for y in 0..sheet.h {
            for x in 0..sheet.w {
                let pixel = sheet.get(x, y);
                assert!(!is_mane(pixel), "char_{index} has a key colour");
                assert!(!COAT.contains(&pixel), "char_{index} has a coat colour");
            }
        }
    }
}

#[test]
fn the_styles_are_different_heads_and_the_first_is_the_classic_lion() {
    let from = source(0);
    let sheets: Vec<Bitmap> = STYLES
        .iter()
        .map(|style| lion_sheet(&from, style))
        .collect();
    for (index, sheet) in sheets.iter().enumerate() {
        for (other, with) in sheets.iter().enumerate().skip(index + 1) {
            // From the front, standing: the frame a lion is mostly seen in.
            assert_ne!(
                frame(sheet, 0, 1),
                frame(with, 0, 1),
                "{} and {} are one head",
                STYLES[index].name,
                STYLES[other].name
            );
        }
    }
    let mut names: Vec<&str> = STYLES.iter().map(|style| style.name).collect();
    names.dedup();
    assert_eq!(names.len(), STYLES.len());
    assert_eq!(STYLES[0].name, "classic");
    // A style changes the head and nothing under it: the legs are the same.
    for sheet in &sheets[1..] {
        for row in 0..3 {
            let (a, b) = (frame(sheet, row, 1), frame(&sheets[0], row, 1));
            assert_eq!(a.part(0, 26, FRAME_W, 6), b.part(0, 26, FRAME_W, 6));
        }
    }
}

#[test]
fn every_frame_of_every_lion_has_its_agents_colour_a_body_and_stands_where_the_source_stood() {
    for index in SOURCES {
        for style in &STYLES {
            let (from, lion) = (source(index), lion_sheet(&source(index), style));
            let name = style.name;
            assert_eq!((lion.w, lion.h), (FRAMES * FRAME_W, 3 * FRAME_H));
            for row in 0..3 {
                for column in 0..FRAMES {
                    let cel = frame(&lion, row, column);
                    let mane = (0..FRAME_H)
                        .flat_map(|y| (0..FRAME_W).map(move |x| (x, y)))
                        .filter(|(x, y)| is_mane(cel.get(*x, *y)))
                        .count();
                    // A full mane is most of the head; a scarf, a crest or a
                    // cropped mane is less, and still plain to see.
                    let least = if style.name == "classic" { 40 } else { 12 };
                    assert!(
                        mane >= least,
                        "char_{index} {name} row {row} frame {column}: {mane} of mane"
                    );
                    // The head is the top of the frame.
                    let top = cel.top().expect("a lion is drawn");
                    assert!(
                        (0..=10).contains(&top),
                        "char_{index} {name} {row}/{column} starts at {top}"
                    );
                    assert!(
                        cel.area() > 180,
                        "char_{index} {name} {row}/{column} is {}",
                        cel.area()
                    );
                }
                // The feet are where the source's feet were: the body is
                // kept. (Seated and seen from behind, the tail hangs lower.)
                let frames = if row == 1 { 0..3 } else { 0..7 };
                for column in frames {
                    let (cel, was) = (frame(&lion, row, column), frame(&from, row, column));
                    let bottom = |picture: &Bitmap| {
                        (0..FRAME_H)
                            .rev()
                            .find(|y| (2..14).any(|x| picture.get(x, *y)[3] > 128))
                    };
                    assert_eq!(
                        bottom(&cel),
                        bottom(&was),
                        "char_{index} {name} {row}/{column}"
                    );
                }
            }
        }
    }
}

#[test]
fn no_skin_is_left_on_a_lion_its_hands_are_fur() {
    for index in SOURCES {
        // The body is the same under every head.
        let lion = lion_sheet(&source(index), &STYLES[0]);
        for y in 0..lion.h {
            for x in 0..lion.w {
                let [r, g, b, a] = lion.get(x, y);
                // The skins of the sources are pinker than any fur: more
                // blue for their red.
                let pink = a > 0 && r > 190 && b >= 130 && r > g && g > b && !is_mane([r, g, b, a]);
                let cream = r > 240 && g > 220;
                assert!(!pink || cream, "char_{index}: skin at {x},{y}: {r},{g},{b}");
            }
        }
    }
}

#[test]
fn the_three_extra_frames_differ_from_the_frames_they_are_made_of_only_by_the_eyes() {
    for style in &STYLES {
        the_extra_frames_are_faces(&lion_sheet(&source(0), style));
    }
}

fn the_extra_frames_are_faces(lion: &Bitmap) {
    for (extra, base) in [(FRAME_ASLEEP, 3), (FRAME_STARE, 1), (FRAME_OUT, 1)] {
        let (a, b) = (frame(lion, 0, extra), frame(lion, 0, base));
        let changed: Vec<(i32, i32)> = (0..FRAME_H)
            .flat_map(|y| (0..FRAME_W).map(move |x| (x, y)))
            .filter(|(x, y)| a.get(*x, *y) != b.get(*x, *y))
            .collect();
        assert!(!changed.is_empty(), "frame {extra} has eyes of its own");
        let rows: Vec<i32> = changed.iter().map(|(_, y)| *y).collect();
        let span = rows.iter().max().unwrap() - rows.iter().min().unwrap();
        assert!(
            span <= 2,
            "frame {extra} changes {span} rows more than the eyes"
        );
        assert!(changed.iter().all(|(x, _)| (3..13).contains(x)));
        // Seen from behind there are no eyes to change.
        assert_eq!(frame(lion, 1, extra), frame(lion, 1, base));
    }
}

#[test]
fn a_walk_moves_the_legs_and_typing_moves_the_arms() {
    for index in SOURCES {
        let lion = lion_sheet(&source(index), &STYLES[0]);
        for row in 0..3 {
            assert_ne!(
                frame(&lion, row, 0),
                frame(&lion, row, 1),
                "char_{index} row {row}"
            );
            assert_ne!(
                frame(&lion, row, 0),
                frame(&lion, row, 2),
                "char_{index} row {row}"
            );
            assert_ne!(
                frame(&lion, row, 3),
                frame(&lion, row, 4),
                "char_{index} row {row}"
            );
        }
    }
}

#[test]
fn the_cub_is_small_has_no_mane_but_a_scarf_and_every_frame_of_it_is_drawn() {
    let cub = cub_sheet();
    assert_eq!((cub.w, cub.h), (CUB_FRAMES * CUB_FRAME, 3 * CUB_FRAME));
    let grown = frame(&lion_sheet(&source(0), &STYLES[1]), 0, 1).area();
    for row in 0..3 {
        for column in 0..CUB_FRAMES {
            let cel = cub.part(column * CUB_FRAME, row * CUB_FRAME, CUB_FRAME, CUB_FRAME);
            assert!(cel.area() > 90, "cub {row}/{column} is {}", cel.area());
            assert!(cel.area() * 3 < grown * 2, "a cub is smaller than a lion");
            let tinted = (0..CUB_FRAME)
                .flat_map(|y| (0..CUB_FRAME).map(move |x| (x, y)))
                .filter(|(x, y)| is_mane(cel.get(*x, *y)))
                .count();
            assert!(
                (4..40).contains(&tinted),
                "cub {row}/{column}: {tinted} in its colour"
            );
        }
        let step =
            |column: i32| cub.part(column * CUB_FRAME, row * CUB_FRAME, CUB_FRAME, CUB_FRAME);
        assert_ne!(step(0), step(1));
        assert_ne!(step(1), step(2));
    }
    // Asleep, staring and out cold are faces of their own.
    let face = |column: i32| cub.part(column * CUB_FRAME, 0, CUB_FRAME, CUB_FRAME);
    assert_ne!(face(5), face(0));
    assert_ne!(face(6), face(0));
    assert_ne!(face(7), face(6));
}
