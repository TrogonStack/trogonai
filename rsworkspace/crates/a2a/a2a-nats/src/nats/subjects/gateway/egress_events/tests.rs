use async_nats::subject::ToSubject;

use super::super::ingress::{GatewayIngressError, GatewayIngressSubject};
use super::*;

fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "alice").unwrap()
}

#[test]
fn renders_the_caller_principal_before_the_events_terminal() {
    let subject = GatewayEgressEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &caller());
    assert_eq!(subject.to_string(), "a2a.v1.gateway.person.alice.events");
    assert_eq!(subject.to_subject().as_str(), "a2a.v1.gateway.person.alice.events");
    assert_eq!(subject.caller(), &caller());
}

#[test]
fn renders_under_a_multi_token_prefix() {
    let subject = GatewayEgressEventsSubject::new(&A2aPrefix::new("my.multi.part").unwrap(), &caller());
    assert_eq!(subject.to_string(), "my.multi.part.v1.gateway.person.alice.events");
}

#[test]
fn is_never_parsed_as_gateway_ingress() {
    let prefix = A2aPrefix::new("a2a").unwrap();
    let subject = GatewayEgressEventsSubject::new(&prefix, &caller());
    assert_eq!(
        GatewayIngressSubject::parse(&prefix, &subject.to_string()),
        Err(GatewayIngressError::BadSubjectShape)
    );
}
