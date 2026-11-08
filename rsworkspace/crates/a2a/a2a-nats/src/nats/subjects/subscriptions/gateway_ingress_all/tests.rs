use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_every_agent_and_caller_under_the_gateway() {
    let s = GatewayIngressAllSubject::new(&A2aPrefix::new("a2a").unwrap());
    assert_eq!(s.to_string(), "a2a.v1.gateway.*.*.*.>");
    assert_eq!(s.to_subject().as_str(), "a2a.v1.gateway.*.*.*.>");
}

#[test]
fn keeps_a_multi_token_prefix() {
    let s = GatewayIngressAllSubject::new(&A2aPrefix::new("my.multi.part").unwrap());
    assert_eq!(s.to_string(), "my.multi.part.v1.gateway.*.*.*.>");
}
