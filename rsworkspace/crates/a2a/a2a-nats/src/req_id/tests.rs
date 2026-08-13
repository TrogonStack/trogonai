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
fn matches_event_headers_compares_the_trimmed_req_id() {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(crate::constants::REQ_ID_HEADER, " req-1 ");
    assert!(ReqId::from_header("req-1").matches_event_headers(Some(&headers)));
    assert!(!ReqId::from_header("req-2").matches_event_headers(Some(&headers)));
}

#[test]
fn matches_event_headers_rejects_events_carrying_no_req_id() {
    assert!(!ReqId::from_header("req-1").matches_event_headers(None));
    let empty = async_nats::HeaderMap::new();
    assert!(!ReqId::from_header("req-1").matches_event_headers(Some(&empty)));
}

#[test]
fn new_is_unique() {
    let a = ReqId::new();
    let b = ReqId::new();
    assert_ne!(a.as_str(), b.as_str());
}
