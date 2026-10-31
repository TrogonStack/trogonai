use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_every_dead_letter_lane_under_the_prefix() {
    let s = PushDlqAllSubject::new(&A2aPrefix::new("a2a").unwrap());
    assert_eq!(s.to_string(), "a2a.v1.push.dlq.>");
    assert_eq!(s.to_subject().as_str(), "a2a.v1.push.dlq.>");
}

#[test]
fn keeps_a_multi_token_prefix() {
    let s = PushDlqAllSubject::new(&A2aPrefix::new("my.multi.part").unwrap());
    assert_eq!(s.to_string(), "my.multi.part.v1.push.dlq.>");
}
