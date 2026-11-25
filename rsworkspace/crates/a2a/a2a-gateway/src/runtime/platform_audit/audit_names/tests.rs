use super::*;

#[test]
fn a_valid_name_is_kept_as_it_is() {
    let name = name_of("message/send@agent-1").expect("name");
    assert_eq!(name.as_str(), "message/send@agent-1");
}

#[test]
fn a_hashed_principal_id_is_kept_as_the_token_the_bus_carries() {
    let principal = trogon_identity_types::PrincipalId::new("alice@example.com").expect("id");
    let token = trogon_identity_types::SubjectPrincipalId::from_credential(&principal).expect("subject id");
    assert!(token.is_hashed(), "{}", token.as_str());
    let name = name_of(token.as_str()).expect("name");
    assert_eq!(name.as_str(), token.as_str());
}

#[test]
fn an_identifier_holding_a_character_a_name_refuses_is_named_by_its_digest() {
    let name = name_of("alice=admin").expect("name");
    assert!(name.as_str().starts_with("sha256:"), "{}", name.as_str());
    assert_eq!(name.as_str().len(), "sha256:".len() + 64);
}

#[test]
fn an_identifier_too_long_for_a_name_is_named_by_its_digest() {
    let long = "a".repeat(300);
    let name = name_of(&long).expect("name");
    assert!(name.as_str().starts_with("sha256:"));
}

#[test]
fn the_same_identifier_is_always_the_same_name() {
    assert_eq!(name_of("alice=1").expect("first"), name_of("alice=1").expect("second"));
    assert_ne!(name_of("alice=1").expect("first"), name_of("alice=2").expect("second"));
}

#[test]
fn the_digest_of_the_empty_identifier_is_still_a_name() {
    let name = name_of("").expect("name");
    assert!(name.as_str().starts_with("sha256:"));
}
