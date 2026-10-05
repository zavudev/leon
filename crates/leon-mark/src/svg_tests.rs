//! Tests of the animated SVG files: they are the app's gestures written for
//! the web, they cannot drift from the code, and their first frame is the
//! lion as the owner drew it.

use crate::geometry::{path_data, shapes, Variant};
use crate::motion::{Gesture, Pose};
use crate::svg::{icon_animated, mark_animated, FRAME_STEP, LOOP_LEN, SCRIPT};

const FINAL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../brand/logo/final");
const FULL_SVG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../app/assets/brand/leon-mark.svg"
);

fn read(path: &str) -> String {
    // Line endings as `\n`, whatever the checkout made of them.
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .replace("\r\n", "\n")
}

/// The colours the final app icon is drawn with: the accent and the ink.
fn brand_colours() -> (String, String) {
    let icon = read(&format!("{FINAL}/app-icon.svg"));
    let document = roxmltree::Document::parse(&icon).expect("the icon is XML");
    let fill = |tag: &str| {
        document
            .descendants()
            .find(|node| node.has_tag_name(tag))
            .and_then(|node| node.attribute("fill"))
            .unwrap_or_else(|| panic!("the icon's {tag} has a fill"))
            .to_owned()
    };
    (fill("path"), fill("rect"))
}

fn rest_d() -> String {
    path_data(&shapes(&Pose::REST, Variant::Full).contours())
}

fn files() -> [(&'static str, String); 2] {
    let (accent, ink) = brand_colours();
    [
        ("mark-animated.svg", mark_animated(&accent)),
        ("app-icon-animated.svg", icon_animated(&accent, &ink)),
    ]
}

/// The `values` of the one animation of a file, as its list of `d` strings.
fn values_of(svg: &str) -> Vec<String> {
    let document = roxmltree::Document::parse(svg).unwrap();
    let animate = document
        .descendants()
        .find(|node| node.has_tag_name("animate"))
        .expect("the file animates");
    animate
        .attribute("values")
        .unwrap()
        .split(';')
        .map(str::to_owned)
        .collect()
}

#[test]
fn both_files_are_well_formed_xml() {
    for (name, svg) in files() {
        roxmltree::Document::parse(&svg).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn the_committed_files_are_what_the_generator_writes_so_they_cannot_drift() {
    for (name, generated) in files() {
        let committed = read(&format!("{FINAL}/{name}"));
        assert_eq!(
            committed.trim_end(),
            generated.trim_end(),
            "{name} is stale: run `cargo run -p leon-mark --example export-svg`"
        );
    }
}

#[test]
fn the_first_and_the_last_frame_are_the_rest_pose_of_leon_mark_svg() {
    let original = read(FULL_SVG);
    let document = roxmltree::Document::parse(&original).unwrap();
    let owners_d = document
        .descendants()
        .find(|node| node.has_tag_name("path"))
        .and_then(|node| node.attribute("d"))
        .unwrap()
        .to_owned();
    assert_eq!(rest_d(), owners_d);
    for (name, svg) in files() {
        let values = values_of(&svg);
        assert_eq!(values.first().unwrap(), &owners_d, "{name}: first frame");
        assert_eq!(values.last().unwrap(), &owners_d, "{name}: last frame");
        // And the path itself, which is what a still renderer draws.
        let document = roxmltree::Document::parse(&svg).unwrap();
        for path in document.descendants().filter(|n| n.has_tag_name("path")) {
            assert_eq!(path.attribute("d").unwrap(), owners_d, "{name}");
            assert_eq!(path.attribute("fill-rule"), Some("evenodd"));
        }
    }
}

#[test]
fn the_animation_has_no_script_and_no_external_reference() {
    for (name, svg) in files() {
        let lower = svg.to_lowercase();
        assert!(!lower.contains("<script"), "{name}");
        assert!(!lower.contains("href"), "{name}");
        assert!(!lower.contains("http://") || lower.matches("http://").count() == 1);
        assert!(!lower.contains("javascript"), "{name}");
        assert!(!lower.contains("onload"), "{name}");
    }
}

#[test]
fn reduced_motion_shows_the_still_lion_through_an_embedded_media_query() {
    for (name, svg) in files() {
        let document = roxmltree::Document::parse(&svg).unwrap();
        let style = document
            .descendants()
            .find(|node| node.has_tag_name("style"))
            .and_then(|node| node.text())
            .unwrap_or_else(|| panic!("{name} has a style"));
        assert!(style.contains("prefers-reduced-motion: reduce"), "{name}");
        let live = document
            .descendants()
            .filter(|node| node.attribute("class") == Some("live"))
            .count();
        let still = document
            .descendants()
            .filter(|node| node.attribute("class") == Some("still"))
            .count();
        assert_eq!((live, still), (1, 1), "{name}");
        // The still one carries no animation at all.
        let still = document
            .descendants()
            .find(|node| node.attribute("class") == Some("still"))
            .unwrap();
        assert!(still.children().all(|child| !child.has_tag_name("animate")));
        // By default only the live one shows.
        assert!(style.contains(".still{display:none}"), "{name}");
    }
}

#[test]
fn the_key_times_start_at_zero_end_at_one_and_only_go_forward() {
    for (name, svg) in files() {
        let document = roxmltree::Document::parse(&svg).unwrap();
        let animate = document
            .descendants()
            .find(|node| node.has_tag_name("animate"))
            .unwrap();
        let times: Vec<f64> = animate
            .attribute("keyTimes")
            .unwrap()
            .split(';')
            .map(|t| t.parse().unwrap())
            .collect();
        assert_eq!(times.len(), values_of(&svg).len(), "{name}");
        assert_eq!(*times.first().unwrap(), 0.);
        assert_eq!(*times.last().unwrap(), 1.);
        assert!(times.windows(2).all(|pair| pair[0] < pair[1]), "{name}");
        assert_eq!(animate.attribute("repeatCount"), Some("indefinite"));
        assert_eq!(
            animate.attribute("dur"),
            Some(format!("{}s", LOOP_LEN).as_str())
        );
        assert_eq!(animate.attribute("attributeName"), Some("d"));
    }
}

#[test]
fn every_frame_of_the_animation_stays_inside_the_64_grid() {
    for (name, svg) in files() {
        for d in values_of(&svg) {
            for number in d.split(['M', 'L', 'Z', ' ']).filter(|n| !n.is_empty()) {
                let value: f32 = number.parse().unwrap();
                assert!((0. ..=64.).contains(&value), "{name}: {value}");
            }
        }
    }
}

#[test]
fn the_loop_plays_the_idle_gestures_with_the_same_timings_as_the_app() {
    // Only the three gestures of the brief, each at least a fifth of a
    // second after the one before, all inside the loop.
    let mut end = 0.;
    for (start, gesture) in SCRIPT {
        assert!(
            matches!(
                gesture,
                Gesture::Blink | Gesture::DoubleBlink | Gesture::Narrow | Gesture::Glance(_)
            ),
            "{gesture:?}"
        );
        assert!(
            start >= end + 0.2,
            "{gesture:?} at {start} follows too close"
        );
        end = start + gesture.duration();
        assert!(end < LOOP_LEN);
    }
    assert!(SCRIPT.iter().any(|(_, g)| *g == Gesture::Blink));
    assert!(SCRIPT.iter().any(|(_, g)| *g == Gesture::DoubleBlink));
    assert!(SCRIPT.iter().any(|(_, g)| *g == Gesture::Narrow));
    assert!(SCRIPT.iter().any(|(_, g)| matches!(g, Gesture::Glance(_))));
}

#[test]
fn a_keyframe_inside_a_gesture_is_the_shape_of_the_gesture_at_that_moment() {
    let (accent, _) = brand_colours();
    let svg = mark_animated(&accent);
    let document = roxmltree::Document::parse(&svg).unwrap();
    let animate = document
        .descendants()
        .find(|node| node.has_tag_name("animate"))
        .unwrap();
    let times: Vec<f32> = animate
        .attribute("keyTimes")
        .unwrap()
        .split(';')
        .map(|t| t.parse::<f32>().unwrap() * LOOP_LEN)
        .collect();
    let values = values_of(&svg);
    let (start, gesture) = SCRIPT[0];
    // The keyframe nearest the middle of the first blink's closing.
    let target = start + 0.07;
    let (index, time) = times
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - target).abs().total_cmp(&(b.1 - target).abs()))
        .unwrap();
    assert!(
        (time - target).abs() <= FRAME_STEP,
        "a keyframe near {target}"
    );
    let expected = path_data(&shapes(&gesture.pose(time - start), Variant::Full).contours());
    assert_eq!(values[index], expected);
}

#[test]
fn the_icon_is_the_lion_on_its_ink_tile_exactly_as_app_icon_svg_places_it() {
    let (accent, ink) = brand_colours();
    let icon = icon_animated(&accent, &ink);
    let original = read(&format!("{FINAL}/app-icon.svg"));
    let ours = roxmltree::Document::parse(&icon).unwrap();
    let theirs = roxmltree::Document::parse(&original).unwrap();
    let attributes =
        |document: &roxmltree::Document, tag: &str, names: &[&str]| -> Vec<Option<String>> {
            let node = document
                .descendants()
                .find(|node| node.has_tag_name(tag))
                .unwrap();
            names
                .iter()
                .map(|name| node.attribute(*name).map(str::to_owned))
                .collect()
        };
    assert_eq!(
        attributes(&ours, "rect", &["width", "height", "rx", "fill"]),
        attributes(&theirs, "rect", &["width", "height", "rx", "fill"]),
    );
    assert_eq!(
        attributes(&ours, "path", &["transform", "fill", "fill-rule", "d"]),
        attributes(&theirs, "path", &["transform", "fill", "fill-rule", "d"]),
    );
}

#[test]
fn the_mark_is_transparent_and_in_the_accent_colour() {
    let (accent, _) = brand_colours();
    let svg = mark_animated(&accent);
    assert!(!svg.contains("<rect"), "no tile, no background");
    let document = roxmltree::Document::parse(&svg).unwrap();
    for path in document.descendants().filter(|n| n.has_tag_name("path")) {
        assert_eq!(path.attribute("fill"), Some(accent.as_str()));
    }
}
