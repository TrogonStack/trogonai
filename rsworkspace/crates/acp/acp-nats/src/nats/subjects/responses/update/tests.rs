use super::*;
use async_nats::subject::ToSubject as _;

#[test]
fn to_subject_matches_display() {
    let prefix = crate::acp_prefix::AcpPrefix::new("acp").expect("prefix");
    let session_id = crate::session_id::AcpSessionId::new("s1").expect("session_id");
    let req_id = crate::req_id::ReqId::from_header("r1");
    let subject = UpdateSubject::new(&prefix, &session_id, &req_id);
    assert_eq!(subject.to_subject().as_str(), subject.to_string());
}
