use std::time::Duration;

use trogon_identity_types::SubjectPrincipal;

use super::mint_test_user_jwt;

#[test]
fn a_test_user_jwt_names_the_caller_as_a_person_and_is_fresh() {
    let minted = mint_test_user_jwt("alice", "tenant-a", Duration::from_secs(60));
    minted.ensure_fresh().expect("fresh jwt");

    let jwt = a2a_identity_types::MintedUserJwt::new(minted.into_string()).expect("compact jwt");
    assert_eq!(jwt.caller_id().expect("caller id").as_str(), "alice");
    assert_eq!(
        jwt.subject_principal().expect("subject principal"),
        SubjectPrincipal::from_tokens("person", "alice").unwrap()
    );
}
