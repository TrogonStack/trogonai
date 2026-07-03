use super::*;

#[test]
fn new_generates_non_empty_id() {
    assert!(!ReqId::new().as_str().is_empty());
}

#[test]
fn default_generates_non_empty_id() {
    assert!(!ReqId::default().as_str().is_empty());
}

#[test]
fn from_header_roundtrips() {
    assert_eq!(ReqId::from_header("xyz").as_str(), "xyz");
}

#[test]
fn display_and_deref() {
    let id = ReqId::from_test("t-1");
    assert_eq!(format!("{id}"), "t-1");
    assert_eq!(id.len(), 3);
    assert!(id.starts_with("t-"));
}

#[test]
fn new_is_unique() {
    let a = ReqId::new();
    let b = ReqId::new();
    assert_ne!(a.as_str(), b.as_str());
}
