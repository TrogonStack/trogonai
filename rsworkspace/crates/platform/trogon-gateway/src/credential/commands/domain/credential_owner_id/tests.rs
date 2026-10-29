use super::*;

#[test]
fn static_config_owner_is_stable() {
    assert_eq!(CredentialOwnerId::static_config().as_str(), "static-config");
}

#[test]
fn rejects_empty_owner_id() {
    assert_eq!(CredentialOwnerId::new(""), Err(CredentialOwnerIdError::Empty));
}
