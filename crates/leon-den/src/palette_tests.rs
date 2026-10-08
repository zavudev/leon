//! Tests of the palette: the Leon themes give chrome that can be read, in
//! dark and in light.

use gpui_kit::{rgb, Hsla};

use crate::palette::{bytes, contrast, mix, scramble, Role};
use crate::testing::{palette, tokens};

#[test]
fn the_chrome_takes_its_page_and_its_signals_straight_from_the_theme() {
    for dark in [true, false] {
        let (t, p) = (tokens(dark), palette(dark));
        assert_eq!(p.ground, t.background);
        assert_eq!(p.signal, t.signal);
        assert_eq!(p.accent, t.accent_fill);
        assert_eq!(
            (p.success, p.warning, p.error, p.info),
            (t.success, t.warning, t.error, t.info)
        );
    }
}

#[test]
fn what_is_written_in_a_bubble_a_plate_or_a_box_can_be_read() {
    for dark in [true, false] {
        let p = palette(dark);
        for (name, ink, fill, least) in [
            ("bubble", p.bubble_ink, p.bubble, 4.5),
            ("name plate", p.bubble, p.ink, 4.5),
            ("selected plate", p.on_accent, p.accent, 4.5),
            ("warning", p.on_status, p.warning, 3.),
            ("error", p.on_status, p.error, 3.),
            ("info", p.on_status, p.info, 3.),
            ("success", p.on_status, p.success, 3.),
            ("text", p.text, p.paper, 4.5),
            ("muted text", p.text_muted, p.paper, 4.5),
            ("selected row", p.text, p.raised, 4.5),
        ] {
            let ratio = contrast(ink, fill);
            assert!(ratio >= least, "dark {dark}: {name} is {ratio}");
        }
    }
}

#[test]
fn every_letter_of_a_glyph_is_a_role_with_a_colour() {
    let p = palette(true);
    for letter in "owbyYSOcarixu".chars() {
        let role = Role::of(letter).unwrap_or_else(|| panic!("{letter} is a role"));
        let _ = p.color(role);
    }
    assert_eq!(Role::of('.'), None);
    assert_eq!(p.color(Role::Accent), p.accent);
    assert_eq!(p.color(Role::Ink), p.ink);
}

#[test]
fn mixing_goes_from_one_colour_to_the_other() {
    let (black, white): (Hsla, Hsla) = (rgb(0x000000).into(), rgb(0xffffff).into());
    assert_eq!(mix(black, white, 0.), black);
    assert_eq!(mix(black, white, 1.), white);
    assert!((mix(black, white, 0.5).l - 0.5).abs() < 0.01);
    assert!((contrast(black, white) - 21.).abs() < 0.01);
    assert_ne!(scramble(1), scramble(2));
    assert_eq!(bytes(rgb(0xd97757).into()), [0xd9, 0x77, 0x57, 255]);
}
