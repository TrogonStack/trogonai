use super::*;

#[test]
fn accepts_uuid_shaped_key_ids() {
    let key_id = PlaidKeyId::new("00000000-0000-4000-8000-000000000001").unwrap();
    assert_eq!(key_id.as_str(), "00000000-0000-4000-8000-000000000001");
    assert_eq!(key_id.to_string(), "00000000-0000-4000-8000-000000000001");
}

#[test]
fn rejects_empty() {
    assert_eq!(PlaidKeyId::new("").unwrap_err(), PlaidKeyIdError::Empty);
    assert_eq!(PlaidKeyIdError::Empty.to_string(), "key id must not be empty");
}

#[test]
fn rejects_over_length() {
    let at_limit = "a".repeat(KEY_ID_LEN_MAX);
    assert!(PlaidKeyId::new(&at_limit).is_ok());

    let err = PlaidKeyId::new("a".repeat(KEY_ID_LEN_MAX + 1)).unwrap_err();
    assert_eq!(err, PlaidKeyIdError::TooLong(KEY_ID_LEN_MAX + 1));
    assert_eq!(err.to_string(), "key id is too long: 129 bytes (max 128)");
}

#[test]
fn equal_ids_hash_alike() {
    let mut set = std::collections::HashSet::new();
    set.insert(PlaidKeyId::new("kid-1").unwrap());
    assert!(set.contains(&PlaidKeyId::new("kid-1").unwrap()));
    assert!(!set.contains(&PlaidKeyId::new("kid-2").unwrap()));
}

#[test]
fn debug_includes_the_identifier() {
    assert_eq!(format!("{:?}", PlaidKeyId::new("kid-1").unwrap()), "PlaidKeyId(kid-1)");
}
