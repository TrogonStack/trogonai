use a2a_nats::nats::subjects::agents::{
    AgentCardSubject, AgentDeliverySubject, MessageSendSubject, MessageStreamSubject, PushDeleteSubject,
    PushGetSubject, PushListSubject, PushSetSubject, TasksCancelSubject, TasksGetSubject, TasksListSubject,
    TasksResubscribeSubject,
};
use a2a_nats::nats::subjects::gateway::{GatewayEgressEventsSubject, GatewayIngressSubject};
use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use a2a_nats::{A2aAgentId, A2aMethod, A2aTaskId};
use trogon_identity_types::{PrincipalId, PrincipalKind};

use super::*;

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn agent() -> A2aAgentId {
    A2aAgentId::new("it").unwrap()
}

fn principal(kind: PrincipalKind, id: &str) -> SubjectPrincipal {
    SubjectPrincipal::from_credential(kind, &PrincipalId::new(id).unwrap()).unwrap()
}

fn caller() -> SubjectPrincipal {
    principal(PrincipalKind::Service, "mtlsstub")
}

fn someone_else() -> SubjectPrincipal {
    principal(PrincipalKind::Service, "someone-else")
}

fn grant(route: AdmissionRoute) -> IssuedPermissions {
    CallerGrant::new(&prefix(), &caller()).permissions(route)
}

fn rendered(patterns: &[SubjectPattern]) -> Vec<&str> {
    patterns.iter().map(SubjectPattern::as_str).collect()
}

fn matches(pattern: &str, subject: &str) -> bool {
    let (mut pattern, mut subject) = (pattern.split('.'), subject.split('.'));
    loop {
        match (pattern.next(), subject.next()) {
            (Some(">"), Some(_)) | (None, None) => return true,
            (Some(expected), Some(token)) if expected == "*" || expected == token => {}
            _ => return false,
        }
    }
}

fn admits(patterns: &[SubjectPattern], subject: &str) -> bool {
    patterns.iter().any(|pattern| matches(pattern.as_str(), subject))
}

fn agent_subjects(caller: &SubjectPrincipal) -> Vec<String> {
    let (prefix, agent) = (prefix(), agent());
    let mut subjects: Vec<String> = A2aMethod::ALL
        .into_iter()
        .map(|method| AgentDeliverySubject::new(&prefix, &agent, caller, method).to_string())
        .collect();
    subjects.extend([
        AgentCardSubject::new(&prefix, &agent, caller).to_string(),
        MessageSendSubject::new(&prefix, &agent, caller).to_string(),
        MessageStreamSubject::new(&prefix, &agent, caller).to_string(),
        PushDeleteSubject::new(&prefix, &agent, caller).to_string(),
        PushGetSubject::new(&prefix, &agent, caller).to_string(),
        PushListSubject::new(&prefix, &agent, caller).to_string(),
        PushSetSubject::new(&prefix, &agent, caller).to_string(),
        TasksCancelSubject::new(&prefix, &agent, caller).to_string(),
        TasksGetSubject::new(&prefix, &agent, caller).to_string(),
        TasksListSubject::new(&prefix, &agent, caller).to_string(),
        TasksResubscribeSubject::new(&prefix, &agent, caller).to_string(),
    ]);
    subjects
}

fn gateway_subjects(caller: &SubjectPrincipal) -> Vec<String> {
    A2aMethod::ALL
        .into_iter()
        .map(|method| GatewayIngressSubject::new(&prefix(), &agent(), caller, method).to_string())
        .collect()
}

fn task_events(caller: &SubjectPrincipal) -> String {
    TaskEventsSubject::new(&prefix(), &A2aTaskId::new("task-1").unwrap(), caller).to_string()
}

#[test]
fn a_direct_caller_publishes_to_agents_and_reads_its_task_events_and_its_own_inbox() {
    let permissions = grant(AdmissionRoute::Direct);
    assert_eq!(
        rendered(&permissions.publish_allow),
        ["a2a.v1.agents.*.service.mtlsstub.>"]
    );
    assert_eq!(
        rendered(&permissions.subscribe_allow),
        ["a2a.v1.tasks.*.service.mtlsstub.events", "_INBOX.service.mtlsstub.>"]
    );
}

#[test]
fn a_bridge_routed_caller_publishes_to_the_gateway_and_reads_the_gateway_fanout() {
    let permissions = grant(AdmissionRoute::BridgeRouted);
    assert_eq!(
        rendered(&permissions.publish_allow),
        ["a2a.v1.gateway.*.service.mtlsstub.>"]
    );
    assert_eq!(
        rendered(&permissions.subscribe_allow),
        [
            "a2a.v1.gateway.service.mtlsstub.events",
            "a2a.v1.tasks.*.service.mtlsstub.events",
            "_INBOX.>"
        ]
    );
}

#[test]
fn a_direct_caller_may_publish_every_agent_subject_that_names_it_and_nothing_else() {
    let publish = grant(AdmissionRoute::Direct).publish_allow;
    for subject in agent_subjects(&caller()) {
        assert!(admits(&publish, &subject), "{subject}");
    }
    for subject in agent_subjects(&someone_else())
        .into_iter()
        .chain(gateway_subjects(&caller()))
    {
        assert!(!admits(&publish, &subject), "{subject}");
    }
}

#[test]
fn a_bridge_routed_caller_may_publish_only_gateway_ingress_that_names_it() {
    let publish = grant(AdmissionRoute::BridgeRouted).publish_allow;
    for subject in gateway_subjects(&caller()) {
        assert!(admits(&publish, &subject), "{subject}");
    }
    for subject in gateway_subjects(&someone_else())
        .into_iter()
        .chain(agent_subjects(&caller()))
    {
        assert!(!admits(&publish, &subject), "{subject}");
    }
}

#[test]
fn either_route_reads_only_its_own_task_events() {
    for route in [AdmissionRoute::Direct, AdmissionRoute::BridgeRouted] {
        let subscribe = grant(route).subscribe_allow;
        assert!(admits(&subscribe, &task_events(&caller())), "{route:?}");
        assert!(!admits(&subscribe, &task_events(&someone_else())), "{route:?}");
    }
}

#[test]
fn only_a_bridge_routed_caller_reads_its_gateway_fanout() {
    let own = GatewayEgressEventsSubject::new(&prefix(), &caller()).to_string();
    let other = GatewayEgressEventsSubject::new(&prefix(), &someone_else()).to_string();
    let bridge_routed = grant(AdmissionRoute::BridgeRouted).subscribe_allow;
    assert!(admits(&bridge_routed, &own));
    assert!(!admits(&bridge_routed, &other));
    assert!(!admits(&grant(AdmissionRoute::Direct).subscribe_allow, &own));
}

#[test]
fn a_direct_caller_takes_replies_only_under_its_own_inbox_prefix() {
    let subscribe = grant(AdmissionRoute::Direct).subscribe_allow;
    assert!(admits(&subscribe, "_INBOX.service.mtlsstub.nuid.1"));
    assert!(!admits(&subscribe, "_INBOX.service.someone-else.nuid.1"));
    assert!(!admits(&subscribe, "_INBOX.nuid.1"));
}

#[test]
fn a_bridge_routed_caller_takes_replies_on_any_inbox() {
    assert!(admits(
        &grant(AdmissionRoute::BridgeRouted).subscribe_allow,
        "_INBOX.nuid.1"
    ));
}

#[test]
fn the_grant_lives_under_the_configured_prefix() {
    let prefix = A2aPrefix::new("acme.a2a").unwrap();
    let permissions = CallerGrant::new(&prefix, &caller()).permissions(AdmissionRoute::Direct);
    assert_eq!(
        rendered(&permissions.publish_allow),
        ["acme.a2a.v1.agents.*.service.mtlsstub.>"]
    );
    assert_eq!(
        rendered(&permissions.subscribe_allow),
        [
            "acme.a2a.v1.tasks.*.service.mtlsstub.events",
            "_INBOX.service.mtlsstub.>"
        ]
    );
}

#[test]
fn a_caller_whose_id_is_not_token_safe_is_named_by_its_hashed_id() {
    let caller = principal(PrincipalKind::Person, "alice.smith");
    assert!(caller.id().is_hashed());
    let permissions = CallerGrant::new(&prefix(), &caller).permissions(AdmissionRoute::Direct);
    assert_eq!(
        rendered(&permissions.publish_allow),
        [format!("a2a.v1.agents.*.person.{}.>", caller.id().as_str())]
    );
}
