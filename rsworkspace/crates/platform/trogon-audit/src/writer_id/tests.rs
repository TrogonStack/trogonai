use super::*;

#[test]
fn is_a_name() {
    let id = WriterId::new("gateway-1").unwrap();
    assert_eq!(id.as_str(), "gateway-1");
    assert_eq!(id.to_string(), "gateway-1");
}

#[test]
fn rejects_what_a_name_rejects() {
    assert_eq!(WriterId::new("a b"), Err(AuditNameError::InvalidCharacter(' ')));
}
