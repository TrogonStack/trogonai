use super::*;

#[test]
fn display_matches_the_wire_name_of_every_status() {
    let expected = [
        (CredentialStatus::Pending, "pending"),
        (CredentialStatus::Active, "active"),
        (CredentialStatus::Previous, "previous"),
        (CredentialStatus::Revoked, "revoked"),
        (CredentialStatus::Expired, "expired"),
        (CredentialStatus::Destroyed, "destroyed"),
    ];

    for (status, name) in expected {
        assert_eq!(status.as_str(), name);
        assert_eq!(status.to_string(), name);
    }
}

#[test]
fn only_active_and_previous_are_readable() {
    assert!(CredentialStatus::Active.is_readable());
    assert!(CredentialStatus::Previous.is_readable());
    assert!(!CredentialStatus::Pending.is_readable());
    assert!(!CredentialStatus::Revoked.is_readable());
    assert!(!CredentialStatus::Expired.is_readable());
    assert!(!CredentialStatus::Destroyed.is_readable());
}

#[test]
fn only_active_is_writable() {
    assert!(CredentialStatus::Active.is_writable());
    assert!(!CredentialStatus::Previous.is_writable());
    assert!(!CredentialStatus::Revoked.is_writable());
}
