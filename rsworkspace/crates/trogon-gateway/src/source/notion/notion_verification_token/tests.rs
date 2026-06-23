use super::*;

#[test]
fn token_roundtrips() {
    let token = NotionVerificationToken::new("secret_example").unwrap();
    assert_eq!(token.as_str(), "secret_example");
}

#[test]
fn debug_redacts() {
    let token = NotionVerificationToken::new("secret_example").unwrap();
    assert_eq!(format!("{token:?}"), "NotionVerificationToken(****)");
}
