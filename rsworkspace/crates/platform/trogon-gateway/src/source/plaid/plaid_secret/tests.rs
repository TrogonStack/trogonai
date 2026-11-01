use super::*;

#[test]
fn accepts_non_empty_secret() {
    let secret = PlaidSecret::new("test-secret").unwrap();
    assert_eq!(secret.as_str(), "test-secret");
}

#[test]
fn rejects_empty_secret() {
    assert!(PlaidSecret::new("").is_err());
}

#[test]
fn debug_redacts() {
    let secret = PlaidSecret::new("test-secret").unwrap();
    assert_eq!(format!("{secret:?}"), "PlaidSecret(****)");
}
