//! Tests of the catalogue: every piece is one the art can draw, and every
//! role a lion needs is served by something.

use std::collections::HashSet;

use crate::assets::art;
use crate::catalogue::{find, Category, Placement, Role, CATALOGUE};
use crate::pose::TILE;

#[test]
fn every_piece_has_an_id_of_its_own_and_is_found_by_it() {
    let mut ids = HashSet::new();
    for entry in CATALOGUE {
        assert!(ids.insert(entry.id), "{} twice", entry.id);
        assert_eq!(find(entry.id).map(|found| found.id), Some(entry.id));
        assert!(
            entry
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{}: an id is written in a file, so it is plain",
            entry.id
        );
        assert!(!entry.name.is_empty());
    }
    assert!(find("throne").is_none());
    assert!(find("").is_none());
}

#[test]
fn every_view_of_every_piece_is_a_picture_as_wide_as_its_footprint() {
    let names: HashSet<&str> = art().sprite_names().collect();
    for entry in CATALOGUE {
        assert!(!entry.views.is_empty(), "{}", entry.id);
        for view in entry.views {
            assert!(
                names.contains(view.sprite),
                "{}: no {}",
                entry.id,
                view.sprite
            );
            let picture = art().sprite(view.sprite);
            assert!(view.w >= 1 && view.h >= 1);
            assert_eq!(picture.w, view.w * TILE, "{} {}", entry.id, view.sprite);
            assert!(
                picture.h <= (view.h + 1) * TILE,
                "{}: {} is taller than its footprint and a tile",
                entry.id,
                view.sprite
            );
        }
        assert!(entry.background < entry.views[0].h.max(1) + 1);
    }
}

#[test]
fn a_piece_turns_through_its_views_and_back_whatever_the_number() {
    for entry in CATALOGUE {
        let views = entry.views.len() as u8;
        assert_eq!(entry.turns(), views > 1, "{}", entry.id);
        for turn in 0..=255u8 {
            assert_eq!(entry.view(turn), &entry.views[(turn % views) as usize]);
        }
    }
    // A chair is seen from four sides, a rack from one.
    assert_eq!(find("chair").unwrap().views.len(), 4);
    assert!(!find("rack").unwrap().turns());
}

#[test]
fn every_category_has_pieces_and_every_role_is_served() {
    for category in Category::ALL {
        assert!(
            CATALOGUE.iter().any(|entry| entry.category == category),
            "{}",
            category.name()
        );
        assert!(!category.name().is_empty());
    }
    for role in Role::ALL {
        assert!(
            CATALOGUE.iter().any(|entry| entry.role == Some(role)),
            "{role:?}"
        );
    }
    // A little seat is a seat.
    for entry in CATALOGUE.iter().filter(|entry| entry.little) {
        assert_eq!(entry.role, Some(Role::Seat), "{}", entry.id);
    }
}

#[test]
fn what_hangs_on_the_wall_fits_the_wall_and_what_lies_flat_is_walked_over() {
    for entry in CATALOGUE {
        match entry.placement {
            Placement::Wall => {
                assert!(entry.views.iter().all(|view| view.h <= 2), "{}", entry.id);
                assert!(!entry.surface);
            }
            Placement::Surface => assert!(!entry.surface, "{}: a thing on a table", entry.id),
            Placement::Floor => {}
        }
        if entry.flat {
            assert!(!entry.solid, "{}: flat and in the way", entry.id);
        }
        if matches!(entry.role, Some(Role::Seat | Role::Nest | Role::Entrance)) {
            assert!(
                !entry.solid,
                "{}: a lion must be able to be on it",
                entry.id
            );
        }
    }
    assert!(find("desk").unwrap().surface && find("desk").unwrap().solid);
}
