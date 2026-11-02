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

fn parse(subject: &str) -> Result<AgentDeliverySubject, AgentDeliverySubjectError> {
    AgentDeliverySubject::parse(&prefix(), subject)
}

#[test]
fn renders_the_caller_between_the_agent_and_the_method() {
    let subject = AgentDeliverySubject::new(&prefix(), &agent(), &caller(), A2aMethod::MessageSend);
    assert_eq!(subject.to_string(), "a2a.v1.agents.planner.person.alice.message.send");
    assert_eq!(
        async_nats::subject::ToSubject::to_subject(&subject).as_str(),
        "a2a.v1.agents.planner.person.alice.message.send"
    );
}

#[test]
fn parse_round_trips_every_method() {
    for method in A2aMethod::ALL {
        let built = AgentDeliverySubject::new(&prefix(), &agent(), &caller(), method);
        assert_eq!(parse(&built.to_string()), Ok(built));
    }
}

#[test]
fn parse_round_trips_under_a_multi_token_prefix() {
    let prefix = A2aPrefix::new("my.multi.part").unwrap();
    let built = AgentDeliverySubject::new(&prefix, &agent(), &caller(), A2aMethod::TasksGet);
    assert_eq!(
        built.to_string(),
        "my.multi.part.v1.agents.planner.person.alice.tasks.get"
    );
    assert_eq!(AgentDeliverySubject::parse(&prefix, &built.to_string()), Ok(built));
}

#[test]
fn parse_round_trips_a_hashed_principal() {
    let id = PrincipalId::new("alice@example.com").unwrap();
    let caller = SubjectPrincipal::from_principal(PrincipalKind::Person, &id);
    assert!(caller.id().is_hashed());
    let built = AgentDeliverySubject::new(&prefix(), &agent(), &caller, A2aMethod::MessageStream);
    let parsed = parse(&built.to_string()).unwrap();
    assert_eq!(parsed.caller(), &caller);
    assert_eq!(
        SubjectPrincipal::from_principal(PrincipalKind::Person, &parsed.caller().principal_id()),
        caller
    );
}

#[test]
fn parse_exposes_the_agent_the_caller_and_the_method() {
    let parsed = parse("a2a.v1.agents.planner.service.billing.push.list").unwrap();
    assert_eq!(parsed.agent_id(), &agent());
    assert_eq!(
        parsed.caller(),
        &SubjectPrincipal::from_tokens("service", "billing").unwrap()
    );
    assert_eq!(parsed.method(), &A2aMethod::PushNotificationList);
}

#[test]
fn parse_rejects_a_foreign_prefix_version_or_family() {
    for subject in [
        "other.v1.agents.planner.person.alice.message.send",
        "a2ax.v1.agents.planner.person.alice.message.send",
        "a2a.v2.agents.planner.person.alice.message.send",
        "a2a.v1.gateway.planner.person.alice.message.send",
        "a2a.v1.agentsx.planner.person.alice.message.send",
        "a2a.v1.agents",
    ] {
        assert_eq!(
            parse(subject),
            Err(AgentDeliverySubjectError::NotAgentDelivery),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_a_subject_without_the_principal_position() {
    for subject in [
        "a2a.v1.agents.planner.message.send",
        "a2a.v1.agents.planner.card",
        "a2a.v1.agents.planner.person.alice",
        "a2a.v1.agents.",
    ] {
        assert_eq!(
            parse(subject),
            Err(AgentDeliverySubjectError::BadSubjectShape),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_an_invalid_agent_id() {
    assert!(matches!(
        parse("a2a.v1.agents.bad*agent.person.alice.message.send"),
        Err(AgentDeliverySubjectError::InvalidAgentId(
            AgentIdError::InvalidCharacter('*')
        ))
    ));
    assert!(matches!(
        parse("a2a.v1.agents..person.alice.message.send"),
        Err(AgentDeliverySubjectError::InvalidAgentId(AgentIdError::Empty))
    ));
}

#[test]
fn parse_rejects_an_invalid_principal_kind() {
    for (kind, expected) in [
        ("robot", PrincipalKindError::Unknown),
        ("*", PrincipalKindError::Unknown),
        ("Person", PrincipalKindError::Unknown),
        ("", PrincipalKindError::Empty),
    ] {
        let subject = format!("a2a.v1.agents.planner.{kind}.alice.message.send");
        assert_eq!(
            parse(&subject),
            Err(AgentDeliverySubjectError::InvalidCaller(SubjectPrincipalError::Kind(
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
        ("al ice", SubjectPrincipalIdError::InvalidCharacter(' ')),
        ("~x", SubjectPrincipalIdError::MalformedDigest),
    ] {
        let subject = format!("a2a.v1.agents.planner.person.{id}.message.send");
        assert_eq!(
            parse(&subject),
            Err(AgentDeliverySubjectError::InvalidCaller(SubjectPrincipalError::Id(
                expected
            ))),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_an_unknown_method() {
    for subject in [
        "a2a.v1.agents.planner.person.alice.message.forward",
        "a2a.v1.agents.planner.person.alice.message.send.extra",
        "a2a.v1.agents.planner.person.alice.>",
    ] {
        assert_eq!(
            parse(subject),
            Err(AgentDeliverySubjectError::UnknownMethod),
            "{subject}"
        );
    }
}
