use super::*;

#[test]
fn accepts_storage_safe_id() {
    let id = CredentialId::new("static-config:github/primary:webhook_secret").unwrap();

    assert_eq!(id.as_str(), "static-config:github/primary:webhook_secret");
}

#[test]
fn rejects_empty_id() {
    assert_eq!(CredentialId::new(""), Err(CredentialIdError::Empty));
}

#[test]
fn rejects_spaces() {
    assert_eq!(
        CredentialId::new("github primary"),
        Err(CredentialIdError::InvalidCharacter(' '))
    );
}

#[test]
fn rejects_id_over_the_maximum_length() {
    let too_long = "a".repeat(MAX_CREDENTIAL_ID_LEN + 1);

    assert_eq!(
        CredentialId::new(too_long),
        Err(CredentialIdError::TooLong(MAX_CREDENTIAL_ID_LEN + 1))
    );
}

#[test]
fn debug_names_the_type_and_value() {
    let id = CredentialId::new("static-config:discord:bot_token").unwrap();

    assert_eq!(format!("{id:?}"), "CredentialId(\"static-config:discord:bot_token\")");
}
