#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{Identity, Owner, identity_allows, owners_allow};

const USER: u32 = 501;
const OTHER_USER: u32 = 502;
const STAFF: u32 = 20;
const WHEEL: u32 = 0;

fn owner(uid: u32, gid: u32, mode: u32) -> Option<Owner> {
    Some(Owner { uid, gid, mode })
}

#[test]
fn the_superuser_can_write_whatever_the_owners_say() {
    assert_eq!(owners_allow(None, None, 0, &[]), Some(true));
    assert_eq!(
        owners_allow(owner(OTHER_USER, WHEEL, 0o644), owner(OTHER_USER, WHEEL, 0o755), 0, &[]),
        Some(true)
    );
}

#[test]
fn owning_the_file_or_the_folder_lets_the_user_write() {
    assert_eq!(
        owners_allow(owner(USER, WHEEL, 0o444), owner(OTHER_USER, WHEEL, 0o755), USER, &[]),
        Some(true)
    );
    assert_eq!(
        owners_allow(owner(OTHER_USER, WHEEL, 0o444), owner(USER, WHEEL, 0o555), USER, &[]),
        Some(true)
    );
}

#[test]
fn a_write_bit_for_others_or_a_shared_group_lets_the_user_write() {
    assert_eq!(
        owners_allow(
            owner(OTHER_USER, WHEEL, 0o646),
            owner(OTHER_USER, WHEEL, 0o755),
            USER,
            &[]
        ),
        Some(true)
    );
    assert_eq!(
        owners_allow(
            owner(OTHER_USER, STAFF, 0o664),
            owner(OTHER_USER, WHEEL, 0o755),
            USER,
            &[STAFF]
        ),
        Some(true)
    );
}

#[test]
fn a_group_write_bit_for_a_group_the_user_lacks_says_no() {
    assert_eq!(
        owners_allow(
            owner(OTHER_USER, STAFF, 0o664),
            owner(OTHER_USER, STAFF, 0o775),
            USER,
            &[WHEEL]
        ),
        Some(false)
    );
}

#[test]
fn an_unknown_owner_leaves_the_answer_open() {
    assert_eq!(owners_allow(owner(OTHER_USER, WHEEL, 0o644), None, USER, &[]), None);
    assert_eq!(owners_allow(None, owner(OTHER_USER, WHEEL, 0o755), USER, &[]), None);
    assert_eq!(owners_allow(None, None, USER, &[]), None);
}

#[test]
fn every_user_id_is_asked_until_one_does_not_say_no() {
    let identity = Identity {
        uids: vec![USER, OTHER_USER],
        groups: Vec::new(),
    };
    assert_eq!(
        identity_allows(&identity, owner(OTHER_USER, WHEEL, 0o644), owner(WHEEL, WHEEL, 0o755)),
        Some(true)
    );
    assert_eq!(
        identity_allows(&identity, owner(WHEEL, WHEEL, 0o644), owner(WHEEL, WHEEL, 0o755)),
        Some(false)
    );
    assert_eq!(identity_allows(&identity, owner(WHEEL, WHEEL, 0o644), None), None);
}
