//! Tests of the built-in dens: each is valid as written, serves the roles,
//! and is a room of its own.

use std::collections::HashSet;

use crate::layout::{DenLayout, MAX_COLS, MAX_ROWS, MIN_COLS, MIN_ROWS};
use crate::prefabs::{default_layout, prefab, prefabs, OFFICE};
use crate::world::{Place, World};

#[test]
fn there_are_at_least_five_and_each_has_an_id_and_a_name_of_its_own() {
    let all = prefabs();
    assert!(all.len() >= 5);
    let ids: HashSet<&str> = all.iter().map(|prefab| prefab.id).collect();
    let names: HashSet<&str> = all
        .iter()
        .map(|prefab| prefab.layout.name.as_str())
        .collect();
    assert_eq!((ids.len(), names.len()), (all.len(), all.len()));
    for one in &all {
        assert_eq!(prefab(one.id).as_ref(), Some(one));
        assert!(
            !one.about.is_empty() && one.about.ends_with('.'),
            "{}",
            one.id
        );
        assert_eq!(one.layout.based_on.as_deref(), Some(one.id));
    }
    assert!(prefab("palace").is_none());
}

#[test]
fn the_default_is_the_office_the_room_everybody_had_before_dens_could_be_chosen() {
    assert_eq!(prefabs()[0].id, OFFICE);
    let office = default_layout();
    assert_eq!((office.cols, office.rows), (20, 15));
    assert_eq!(
        (office.floor.as_str(), office.wall.as_str()),
        ("wood", "rock")
    );
}

#[test]
fn every_prefab_is_valid_as_written() {
    for prefab in prefabs() {
        let room = &prefab.layout;
        assert!((MIN_COLS..=MAX_COLS).contains(&room.cols), "{}", prefab.id);
        assert!((MIN_ROWS..=MAX_ROWS).contains(&room.rows), "{}", prefab.id);
        let again = room.clone().repaired();
        assert_eq!(again.notes, Vec::<String>::new(), "{}", prefab.id);
        assert_eq!(&again.layout, room, "{}", prefab.id);
        // And as it is written to a file and read back.
        let read = DenLayout::load(&room.to_json(), &default_layout());
        assert_eq!((&read.layout, read.notes.len()), (room, 0), "{}", prefab.id);
    }
}

#[test]
fn every_prefab_serves_every_role() {
    for prefab in prefabs() {
        let world = World::build(&prefab.layout);
        // The smallest has no seat for the little ones: they use a lion's.
        let allowed: &[Place] = if prefab.id == "nook" {
            &[Place::Bench]
        } else {
            &[]
        };
        assert_eq!(world.missing(), allowed, "{}", prefab.id);
        for place in Place::ALL {
            if !allowed.contains(&place) {
                assert!(!world.spots(place).is_empty(), "{}: {place:?}", prefab.id);
            }
        }
    }
}

#[test]
fn the_prefabs_differ_in_size_and_in_how_many_can_work_there() {
    let seats = |id: &str| {
        World::build(&prefab(id).unwrap().layout)
            .spots(Place::Desks)
            .len()
    };
    assert_eq!(seats("office"), 12);
    assert!(seats("open-plan") > seats("office"));
    assert!(seats("nook") < seats("office"));
    let sizes: HashSet<(i32, i32)> = prefabs()
        .iter()
        .map(|prefab| (prefab.layout.cols, prefab.layout.rows))
        .collect();
    assert!(sizes.len() >= 5, "{sizes:?}");
    let looks: HashSet<(String, String)> = prefabs()
        .iter()
        .map(|prefab| (prefab.layout.floor.clone(), prefab.layout.wall.clone()))
        .collect();
    assert!(looks.len() >= 5, "{looks:?}");
    let capacities: HashSet<usize> = prefabs().iter().map(|prefab| seats(prefab.id)).collect();
    assert!(capacities.len() >= 4, "{capacities:?}");
}
