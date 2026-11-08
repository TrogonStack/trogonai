use async_nats::subject::ToSubject;
use trogon_identity_types::{PrincipalId, PrincipalKind, PrincipalKindError, SubjectPrincipalIdError};

use super::*;

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn agent() -> A2aAgentId {
    A2aAgentId::new("planner").unwrap()
}

fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "alice").unwrap()
}

fn parse(subject: &str) -> Result<GatewayIngressSubject, GatewayIngressError> {
    GatewayIngressSubject::parse(&prefix(), subject)
}

#[test]
fn renders_the_caller_between_the_agent_and_the_method() {
    let subject = GatewayIngressSubject::new(&prefix(), &agent(), &caller(), A2aMethod::MessageSend);
    assert_eq!(subject.to_string(), "a2a.v1.gateway.planner.person.alice.message.send");
    assert_eq!(
        subject.to_subject().as_str(),
        "a2a.v1.gateway.planner.person.alice.message.send"
    );
}

#[test]
fn parse_round_trips_every_method() {
    for method in A2aMethod::ALL {
        let built = GatewayIngressSubject::new(&prefix(), &agent(), &caller(), method);
        assert_eq!(parse(&built.to_string()), Ok(built));
    }
}

#[test]
fn parse_round_trips_under_a_multi_token_prefix() {
    let prefix = A2aPrefix::new("my.multi.part").unwrap();
    let built = GatewayIngressSubject::new(&prefix, &agent(), &caller(), A2aMethod::PushNotificationSet);
    assert_eq!(
        built.to_string(),
        "my.multi.part.v1.gateway.planner.person.alice.push.set"
    );
    assert_eq!(GatewayIngressSubject::parse(&prefix, &built.to_string()), Ok(built));
}

#[test]
fn parse_round_trips_a_hashed_principal() {
    let caller = SubjectPrincipal::from_principal(PrincipalKind::Service, &PrincipalId::new("svc.billing").unwrap());
    assert!(caller.id().is_hashed());
    let built = GatewayIngressSubject::new(&prefix(), &agent(), &caller, A2aMethod::TasksCancel);
    assert_eq!(parse(&built.to_string()).unwrap().caller(), &caller);
}

#[test]
fn agent_delivery_relays_the_principal_parsed_from_the_ingress_subject() {
    let ingress = parse("a2a.v1.gateway.planner.service.billing.tasks.resubscribe").unwrap();
    let delivery = ingress.agent_delivery();
    assert_eq!(
        delivery.to_string(),
        "a2a.v1.agents.planner.service.billing.tasks.resubscribe"
    );
    assert_eq!(delivery.agent_id(), ingress.agent_id());
    assert_eq!(delivery.caller(), ingress.caller());
    assert_eq!(delivery.method(), ingress.method());
}

#[test]
fn agent_delivery_keeps_the_prefix() {
    let prefix = A2aPrefix::new("my.app").unwrap();
    let ingress = GatewayIngressSubject::parse(&prefix, "my.app.v1.gateway.planner.agent.scout.card").unwrap();
    assert_eq!(
        ingress.agent_delivery().to_string(),
        "my.app.v1.agents.planner.agent.scout.card"
    );
}

#[test]
fn parse_rejects_a_foreign_prefix_version_or_family() {
    for subject in [
        "other.v1.gateway.planner.person.alice.message.send",
        "a2a.v2.gateway.planner.person.alice.message.send",
        "a2a.v1.agents.planner.person.alice.message.send",
        "a2a.v1.gatewayx.planner.person.alice.message.send",
        "a2a.v1.gateway",
    ] {
        assert_eq!(parse(subject), Err(GatewayIngressError::NotGatewayIngress), "{subject}");
    }
}

#[test]
fn parse_rejects_a_subject_without_the_principal_position() {
    for subject in [
        "a2a.v1.gateway.planner.message.send",
        "a2a.v1.gateway.planner.card",
        "a2a.v1.gateway.person.alice.events",
        "a2a.v1.gateway.",
    ] {
        assert_eq!(parse(subject), Err(GatewayIngressError::BadSubjectShape), "{subject}");
    }
}

#[test]
fn parse_rejects_an_invalid_agent_id() {
    assert!(matches!(
        parse("a2a.v1.gateway.bad*agent.person.alice.message.send"),
        Err(GatewayIngressError::InvalidAgentId(AgentIdError::InvalidCharacter('*')))
    ));
}

#[test]
fn parse_rejects_an_invalid_principal_kind() {
    for (kind, expected) in [
        ("robot", PrincipalKindError::Unknown),
        ("*", PrincipalKindError::Unknown),
        ("", PrincipalKindError::Empty),
    ] {
        let subject = format!("a2a.v1.gateway.planner.{kind}.alice.message.send");
        assert_eq!(
            parse(&subject),
            Err(GatewayIngressError::InvalidCaller(SubjectPrincipalError::Kind(
                expected
            ))),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_an_invalid_principal_id() {
    for (id, expected) in [
        ("", SubjectPrincipalIdError::Empty),
        ("*", SubjectPrincipalIdError::InvalidCharacter('*')),
        (">", SubjectPrincipalIdError::InvalidCharacter('>')),
        ("~x", SubjectPrincipalIdError::MalformedDigest),
    ] {
        let subject = format!("a2a.v1.gateway.planner.person.{id}.message.send");
        assert_eq!(
            parse(&subject),
            Err(GatewayIngressError::InvalidCaller(SubjectPrincipalError::Id(expected))),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_an_unknown_method() {
    for subject in [
        "a2a.v1.gateway.planner.person.alice.message.sned",
        "a2a.v1.gateway.planner.person.alice.tasks",
        "a2a.v1.gateway.planner.person.alice.message.send.extra",
    ] {
        assert_eq!(
            parse(subject),
            Err(GatewayIngressError::UnknownMethodSuffix),
            "{subject}"
        );
    }
}

#[test]
fn error_messages_name_the_expected_shape() {
    assert_eq!(
        GatewayIngressError::NotGatewayIngress.to_string(),
        "subject does not start with '{prefix}.v1.gateway.' for the configured prefix"
    );
    assert_eq!(
        GatewayIngressError::BadSubjectShape.to_string(),
        "expected '{prefix}.v1.gateway.{agent_id}.{kind}.{id}.{method…}'"
    );
    assert_eq!(
        GatewayIngressError::UnknownMethodSuffix.to_string(),
        "unknown method suffix after the caller principal"
    );
    assert_eq!(
        GatewayIngressError::InvalidAgentId(AgentIdError::Empty).to_string(),
        "agent id segment fails NATS token validation"
    );
    assert_eq!(
        GatewayIngressError::InvalidCaller(SubjectPrincipalError::Kind(PrincipalKindError::Unknown)).to_string(),
        "caller principal tokens are invalid"
    );
}
