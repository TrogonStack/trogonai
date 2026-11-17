//! ADR#0055 conformance over the whole A2A subject inventory.
//!
//! Every subject type is constructed here and checked against the shared
//! validator. A new subject type that skips this list is the failure mode this
//! guards against, so [`published`] and [`patterns`] are the single place to
//! register one. The matching tests pin which published subjects each role's
//! pattern captures, so a grammar change that strands a subscriber, or lets one
//! read another caller's subjects, fails here (ADR#0082).

use trogon_identity_types::{PrincipalId, PrincipalKind, SubjectPrincipal};
use trogon_nats::subject_conformance::{
    looks_like_request_id, validate_binding_version, validate_published_subject, validate_subject_pattern,
};

use super::{A2aStream, agents, gateway, push, subscriptions, tasks};
use crate::a2a_method::A2aMethod;
use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::task_id::A2aTaskId;

const PREFIXES: [&str; 2] = ["a2a", "my.multi.part"];

/// Subject types that still carry a per-request id in the subject.
///
/// ADR#0055 keeps correlation in headers, and no A2A subject violates that any
/// more. The assertion below pins this set exactly, so a new violation fails
/// this test. Do not grow it.
const REQ_ID_IN_SUBJECT_ALLOWLIST: [&str; 0] = [];

fn prefix(s: &str) -> A2aPrefix {
    A2aPrefix::new(s).expect("test prefix")
}

fn agent() -> A2aAgentId {
    A2aAgentId::new("agent_01").expect("test agent id")
}

fn other_agent() -> A2aAgentId {
    A2aAgentId::new("agent_02").expect("test agent id")
}

/// Deliberately not UUID-shaped. A task id is a durable resource identity, and
/// the request-id ratchet below must not confuse the two.
fn task() -> A2aTaskId {
    A2aTaskId::new("task_01").expect("test task id")
}

/// An id that is already a lower_snake token, so a pattern scoped to it still
/// passes the stricter pattern validation.
fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "caller_01").expect("test principal")
}

/// An id that is not a single token, so every subject carries its hashed form.
fn hashed_caller() -> SubjectPrincipal {
    SubjectPrincipal::from_principal(
        PrincipalKind::Service,
        &PrincipalId::new("svc.billing").expect("test principal id"),
    )
}

fn callers() -> [SubjectPrincipal; 2] {
    [caller(), hashed_caller()]
}

/// Every per-operation subject paired with the method whose delivery it names.
fn per_operation(p: &A2aPrefix, a: &A2aAgentId, c: &SubjectPrincipal) -> Vec<(&'static str, A2aMethod, String)> {
    vec![
        (
            "AgentCardSubject",
            A2aMethod::AgentCard,
            agents::AgentCardSubject::new(p, a, c).to_string(),
        ),
        (
            "MessageSendSubject",
            A2aMethod::MessageSend,
            agents::MessageSendSubject::new(p, a, c).to_string(),
        ),
        (
            "MessageStreamSubject",
            A2aMethod::MessageStream,
            agents::MessageStreamSubject::new(p, a, c).to_string(),
        ),
        (
            "PushSetSubject",
            A2aMethod::PushNotificationSet,
            agents::PushSetSubject::new(p, a, c).to_string(),
        ),
        (
            "PushGetSubject",
            A2aMethod::PushNotificationGet,
            agents::PushGetSubject::new(p, a, c).to_string(),
        ),
        (
            "PushListSubject",
            A2aMethod::PushNotificationList,
            agents::PushListSubject::new(p, a, c).to_string(),
        ),
        (
            "PushDeleteSubject",
            A2aMethod::PushNotificationDelete,
            agents::PushDeleteSubject::new(p, a, c).to_string(),
        ),
        (
            "TasksCancelSubject",
            A2aMethod::TasksCancel,
            agents::TasksCancelSubject::new(p, a, c).to_string(),
        ),
        (
            "TasksGetSubject",
            A2aMethod::TasksGet,
            agents::TasksGetSubject::new(p, a, c).to_string(),
        ),
        (
            "TasksListSubject",
            A2aMethod::TasksList,
            agents::TasksListSubject::new(p, a, c).to_string(),
        ),
        (
            "TasksResubscribeSubject",
            A2aMethod::TasksResubscribe,
            agents::TasksResubscribeSubject::new(p, a, c).to_string(),
        ),
    ]
}

/// Every concrete subject a binding publishes to, paired with its type name.
fn published(p: &A2aPrefix) -> Vec<(&'static str, String)> {
    let a = agent();
    let mut out = Vec::new();
    for c in callers() {
        out.extend(
            per_operation(p, &a, &c)
                .into_iter()
                .map(|(name, _, subject)| (name, subject)),
        );
        for method in A2aMethod::ALL {
            out.push((
                "AgentDeliverySubject",
                agents::AgentDeliverySubject::new(p, &a, &c, method.clone()).to_string(),
            ));
            out.push((
                "GatewayIngressSubject",
                gateway::GatewayIngressSubject::new(p, &a, &c, method).to_string(),
            ));
        }
        out.push((
            "TaskEventsSubject",
            tasks::TaskEventsSubject::new(p, &task(), &c).to_string(),
        ));
        out.push((
            "GatewayEgressEventsSubject",
            gateway::GatewayEgressEventsSubject::new(p, &c).to_string(),
        ));
        let dead_letter = push::PushDlqSubject::new(p, &c, &task());
        out.push(("PushDlqMirrorSubject", dead_letter.mirror().to_string()));
        out.push(("PushDlqSubject", dead_letter.to_string()));
    }
    out
}

/// Every subscription pattern, plus every stream's captured subjects.
fn patterns(p: &A2aPrefix) -> Vec<(&'static str, String)> {
    let mut out = vec![
        (
            "TaskAllEventsSubject",
            subscriptions::TaskAllEventsSubject::new(p).to_string(),
        ),
        (
            "AgentAllSubject",
            subscriptions::AgentAllSubject::new(p, &agent()).to_string(),
        ),
        (
            "GatewayIngressAllSubject",
            subscriptions::GatewayIngressAllSubject::new(p).to_string(),
        ),
        (
            "CallerTaskEventsSubject",
            subscriptions::CallerTaskEventsSubject::new(p, &caller()).to_string(),
        ),
        (
            "PushDlqAllSubject",
            subscriptions::PushDlqAllSubject::new(p).to_string(),
        ),
    ];

    for stream in A2aStream::ALL {
        out.extend(stream.subject_patterns(p).into_iter().map(|s| ("A2aStream", s)));
    }
    out
}

/// NATS subject matching: `*` takes exactly one token, `>` one or more trailing tokens.
fn nats_subject_matches(pattern: &str, subject: &str) -> bool {
    let mut subject_tokens = subject.split('.');
    for token in pattern.split('.') {
        match token {
            ">" => return subject_tokens.next().is_some(),
            "*" => {
                if subject_tokens.next().is_none() {
                    return false;
                }
            }
            literal => {
                if subject_tokens.next() != Some(literal) {
                    return false;
                }
            }
        }
    }
    subject_tokens.next().is_none()
}

#[test]
fn every_published_subject_conforms() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for (name, subject) in published(&p) {
            assert_eq!(validate_published_subject(&subject), Ok(()), "{name}: {subject}");
        }
    }
}

#[test]
fn every_pattern_conforms() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for (name, subject) in patterns(&p) {
            assert_eq!(validate_subject_pattern(&subject), Ok(()), "{name}: {subject}");
        }
    }
}

#[test]
fn every_subject_carries_the_binding_version() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for (name, subject) in published(&p).into_iter().chain(patterns(&p)) {
            assert_eq!(
                validate_binding_version(&subject, prefix_str),
                Ok(()),
                "{name}: {subject}"
            );
        }
    }
}

#[test]
fn no_new_subject_type_embeds_a_request_id() {
    let p = prefix("a2a");
    let mut offenders: Vec<&str> = published(&p)
        .into_iter()
        .filter(|(_, subject)| subject.split('.').any(looks_like_request_id))
        .map(|(name, _)| name)
        .collect();
    offenders.sort_unstable();
    offenders.dedup();

    let mut expected = REQ_ID_IN_SUBJECT_ALLOWLIST.to_vec();
    expected.sort_unstable();

    assert_eq!(
        offenders, expected,
        "the set of subjects embedding a request id changed; \
         shrink REQ_ID_IN_SUBJECT_ALLOWLIST when one is fixed, and never grow it"
    );
}

#[test]
fn every_per_operation_subject_is_the_delivery_of_its_method() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for c in callers() {
            let per_op = per_operation(&p, &agent(), &c);
            assert_eq!(per_op.len(), A2aMethod::ALL.len());
            for (name, method, subject) in per_op {
                let delivery = agents::AgentDeliverySubject::new(&p, &agent(), &c, method);
                assert_eq!(delivery.to_string(), subject, "{name}");
            }
        }
    }
}

#[test]
fn the_agent_subscription_captures_deliveries_from_every_caller_and_only_its_own() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        let pattern = subscriptions::AgentAllSubject::new(&p, &agent()).to_string();
        for c in callers() {
            for method in A2aMethod::ALL {
                let own = agents::AgentDeliverySubject::new(&p, &agent(), &c, method.clone()).to_string();
                assert!(nats_subject_matches(&pattern, &own), "{pattern} must match {own}");
                let other = agents::AgentDeliverySubject::new(&p, &other_agent(), &c, method.clone()).to_string();
                assert!(
                    !nats_subject_matches(&pattern, &other),
                    "{pattern} must not match {other}"
                );
                let ingress = gateway::GatewayIngressSubject::new(&p, &agent(), &c, method).to_string();
                assert!(
                    !nats_subject_matches(&pattern, &ingress),
                    "{pattern} must not match {ingress}"
                );
            }
        }
    }
}

#[test]
fn the_gateway_subscription_captures_every_ingress_and_neither_deliveries_nor_its_own_egress() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        let pattern = subscriptions::GatewayIngressAllSubject::new(&p).to_string();
        for c in callers() {
            for method in A2aMethod::ALL {
                let ingress = gateway::GatewayIngressSubject::new(&p, &agent(), &c, method.clone()).to_string();
                assert!(
                    nats_subject_matches(&pattern, &ingress),
                    "{pattern} must match {ingress}"
                );
                let delivery = agents::AgentDeliverySubject::new(&p, &agent(), &c, method).to_string();
                assert!(
                    !nats_subject_matches(&pattern, &delivery),
                    "{pattern} must not match {delivery}"
                );
            }
            let egress = gateway::GatewayEgressEventsSubject::new(&p, &c).to_string();
            assert!(
                !nats_subject_matches(&pattern, &egress),
                "{pattern} must not match {egress}"
            );
        }
    }
}

#[test]
fn the_events_stream_captures_task_events_from_every_caller() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        let captured: Vec<String> = A2aStream::ALL
            .iter()
            .flat_map(|stream| stream.subject_patterns(&p))
            .collect();
        let pattern = subscriptions::TaskAllEventsSubject::new(&p).to_string();
        assert!(captured.contains(&pattern), "{captured:?} must contain {pattern}");
        for c in callers() {
            let subject = tasks::TaskEventsSubject::new(&p, &task(), &c).to_string();
            assert!(
                nats_subject_matches(&pattern, &subject),
                "{pattern} must match {subject}"
            );
        }
    }
}

#[test]
fn a_caller_scoped_event_filter_captures_only_that_callers_task_events() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        let [first, second] = callers();
        for (own, foreign) in [(&first, &second), (&second, &first)] {
            let pattern = subscriptions::CallerTaskEventsSubject::new(&p, own).to_string();
            let mine = tasks::TaskEventsSubject::new(&p, &task(), own).to_string();
            assert!(nats_subject_matches(&pattern, &mine), "{pattern} must match {mine}");
            let theirs = tasks::TaskEventsSubject::new(&p, &task(), foreign).to_string();
            assert!(
                !nats_subject_matches(&pattern, &theirs),
                "{pattern} must not match {theirs}"
            );
        }
    }
}

#[test]
fn the_push_dlq_stream_keeps_dead_letters_and_their_mirrors_in_separate_lanes() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        let patterns = A2aStream::PushDlq.subject_patterns(&p);
        let [authoritative, mirror_lane] = patterns.as_slice() else {
            panic!("expected an authoritative and a mirror pattern, got {patterns:?}");
        };
        let consumer = subscriptions::PushDlqAllSubject::new(&p).to_string();
        for c in callers() {
            let dead_letter = push::PushDlqSubject::new(&p, &c, &task());
            let original = dead_letter.to_string();
            let mirror = dead_letter.mirror().to_string();
            assert!(
                nats_subject_matches(authoritative, &original),
                "{authoritative} must match {original}"
            );
            assert!(
                !nats_subject_matches(authoritative, &mirror),
                "{authoritative} must not match {mirror}"
            );
            assert!(
                nats_subject_matches(mirror_lane, &mirror),
                "{mirror_lane} must match {mirror}"
            );
            assert!(
                !nats_subject_matches(mirror_lane, &original),
                "{mirror_lane} must not match {original}"
            );
            assert!(
                nats_subject_matches(&consumer, &original),
                "{consumer} must match {original}"
            );
            assert!(
                nats_subject_matches(&consumer, &mirror),
                "{consumer} must match {mirror}"
            );
        }
    }
}

#[test]
fn the_subject_matcher_follows_nats_wildcard_rules() {
    assert!(nats_subject_matches("a.*.c", "a.b.c"));
    assert!(!nats_subject_matches("a.*.c", "a.b.d.c"));
    assert!(nats_subject_matches("a.>", "a.b.c"));
    assert!(!nats_subject_matches("a.>", "a"));
    assert!(!nats_subject_matches("a.b", "a.b.c"));
    assert!(!nats_subject_matches("a.b.c", "a.b"));
}
