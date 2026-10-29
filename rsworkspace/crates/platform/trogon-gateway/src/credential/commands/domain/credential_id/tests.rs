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
