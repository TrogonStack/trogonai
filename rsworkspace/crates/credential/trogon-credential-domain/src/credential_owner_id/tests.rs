use super::*;

#[test]
fn static_config_owner_is_stable() {
    assert_eq!(CredentialOwnerId::static_config().as_str(), "static-config");
}

#[test]
fn rejects_empty_owner_id() {
    assert_eq!(CredentialOwnerId::new(""), Err(CredentialOwnerIdError::Empty));
}

#[test]
fn rejects_owner_id_over_the_maximum_length() {
    let too_long = "a".repeat(MAX_OWNER_ID_LEN + 1);

    assert_eq!(
        CredentialOwnerId::new(too_long),
        Err(CredentialOwnerIdError::TooLong(MAX_OWNER_ID_LEN + 1))
    );
}

#[test]
fn rejects_characters_outside_the_owner_alphabet() {
    assert_eq!(
        CredentialOwnerId::new("tenant/1"),
        Err(CredentialOwnerIdError::InvalidCharacter('/'))
    );
}

#[test]
fn debug_names_the_type_and_value() {
    let owner = CredentialOwnerId::new("tenant-1").unwrap();

    assert_eq!(format!("{owner:?}"), "CredentialOwnerId(\"tenant-1\")");
}

#[test]
fn rejects_dot_path_segments_so_the_owner_stays_inside_its_subtree() {
    assert_eq!(CredentialOwnerId::new("."), Err(CredentialOwnerIdError::DotSegment));
    assert_eq!(CredentialOwnerId::new(".."), Err(CredentialOwnerIdError::DotSegment));
    assert!(CredentialOwnerId::new("tenant.1").is_ok());
}
