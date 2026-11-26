use super::*;

#[test]
fn initial_version_is_one() {
    assert_eq!(CredentialVersion::initial().get(), 1);
}

#[test]
fn rejects_zero() {
    assert_eq!(CredentialVersion::new(0), Err(CredentialVersionError::Zero));
}
