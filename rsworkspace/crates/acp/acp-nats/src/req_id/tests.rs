use super::*;

#[test]
fn new_generates_non_empty_id() {
    let id = ReqId::new();
    assert!(!id.as_str().is_empty());
}

#[test]
fn default_generates_non_empty_id() {
    let id = ReqId::default();
    assert!(!id.as_str().is_empty());
}

#[test]
fn from_header_roundtrips() {
    let id = ReqId::from_header("some-id");
    assert_eq!(id.as_str(), "some-id");
}

#[test]
fn display_and_deref() {
    let id = ReqId::from_test("test-id");
    assert_eq!(format!("{}", id), "test-id");
    assert_eq!(id.len(), 7);
    assert!(id.starts_with("test"));
}
