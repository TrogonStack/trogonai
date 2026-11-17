use super::*;

#[test]
fn says_nothing_beyond_the_category() {
    assert_eq!(CredentialReleaseError::Denied.to_string(), "credential release denied");
    assert_eq!(CredentialReleaseError::KeyDenied.to_string(), "requester xkey denied");
    assert_eq!(
        CredentialReleaseError::Unavailable.to_string(),
        "credential release is unavailable"
    );
    assert_eq!(
        CredentialReleaseError::IntegrityFailure.to_string(),
        "credential release failed an integrity check"
    );
    assert_eq!(
        CredentialReleaseError::Internal.to_string(),
        "credential release failed internally"
    );
}
