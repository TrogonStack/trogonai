use super::*;

#[test]
fn accepts_non_empty_token() {
    let token = ZoomSecretToken::new("test-token").unwrap();
    assert_eq!(token.as_str(), "test-token");
}

#[test]
fn rejects_empty_token() {
    assert!(ZoomSecretToken::new("").is_err());
}

#[test]
fn debug_redacts() {
    let token = ZoomSecretToken::new("test-token").unwrap();
    assert_eq!(format!("{token:?}"), "ZoomSecretToken(****)");
}
