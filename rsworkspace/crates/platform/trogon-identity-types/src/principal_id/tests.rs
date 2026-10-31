use super::{PrincipalId, PrincipalIdError};

#[test]
fn every_way_of_naming_a_principal_id_names_the_same_one() {
    let expected = PrincipalId::new("agent-1").expect("valid principal id");

    assert_eq!("agent-1".parse::<PrincipalId>(), Ok(expected.clone()));
    assert_eq!(PrincipalId::try_from("agent-1"), Ok(expected.clone()));
    assert_eq!(PrincipalId::try_from("agent-1".to_owned()), Ok(expected.clone()));
    assert_eq!(expected.to_string().parse::<PrincipalId>(), Ok(expected.clone()));
    assert_eq!(expected.as_str(), "agent-1");
}

#[test]
fn a_principal_id_rejects_the_empty_string() {
    assert_eq!(PrincipalId::new(""), Err(PrincipalIdError::Empty));
}

#[test]
fn a_principal_id_rejects_control_characters() {
    assert_eq!(
        PrincipalId::new("agent\u{0}1"),
        Err(PrincipalIdError::ContainsControlCharacter)
    );
}

#[test]
fn a_principal_id_keeps_characters_a_subject_token_cannot_carry() {
    let id = PrincipalId::new("alice.smith@example.com").expect("valid principal id");

    assert_eq!(id.as_str(), "alice.smith@example.com");
}
