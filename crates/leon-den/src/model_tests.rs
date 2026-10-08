//! Tests of the model: the words of a state, and what two lists of lions
//! tell.

use crate::model::{happenings_between, CubState, Event, Status};
use crate::testing::cub;

#[test]
fn every_state_has_plain_words_and_a_short_tag() {
    for state in CubState::ALL {
        assert!(!state.label().is_empty());
        assert!(
            !state.label().contains('!'),
            "{state:?}: the truth is sober"
        );
        let tag = state.tag();
        assert!((2..=5).contains(&tag.chars().count()), "{state:?}: {tag}");
        assert_eq!(tag, tag.to_uppercase());
    }
}

#[test]
fn only_what_is_urgent_or_unknown_carries_a_status() {
    assert_eq!(CubState::NeedsPermission.status(), Some(Status::Warning));
    assert_eq!(CubState::Fainted.status(), Some(Status::Error));
    assert_eq!(CubState::WaitingForUser.status(), Some(Status::Attention));
    assert_eq!(CubState::Mystery.status(), Some(Status::Info));
    assert_eq!(CubState::Editing.status(), None);
    assert!(CubState::Editing.is_working() && CubState::Mystery.is_working());
    assert!(!CubState::Idle.is_working() && !CubState::WaitingForUser.is_working());
}

#[test]
fn two_lists_tell_who_joined_who_left_and_what_became_urgent() {
    let before = vec![
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Idle),
        cub(3, "ash", CubState::Running),
        cub(4, "wren", CubState::Thinking),
    ];
    let after = vec![
        cub(1, "moss", CubState::WaitingForUser),
        cub(2, "fern", CubState::Mystery),
        cub(3, "ash", CubState::Fainted),
        cub(5, "pike", CubState::NeedsPermission),
    ];
    let told: Vec<(u64, Event)> = happenings_between(&before, &after)
        .into_iter()
        .map(|happening| (happening.cub, happening.event))
        .collect();
    assert_eq!(
        told,
        vec![
            (1, Event::TurnEnded),
            (2, Event::Mysterious),
            (3, Event::Fainted { exit: None }),
            (5, Event::Joined),
            (5, Event::PermissionPrompt),
            (4, Event::WentHome),
        ]
    );
}

#[test]
fn nothing_is_told_when_nothing_changed_and_no_tool_is_made_up() {
    let before = vec![cub(1, "moss", CubState::Editing)];
    assert!(happenings_between(&before, &before).is_empty());
    // From one tool to another: the lists do not know the tool.
    let after = vec![cub(1, "moss", CubState::Running)];
    assert!(happenings_between(&before, &after).is_empty());
}
