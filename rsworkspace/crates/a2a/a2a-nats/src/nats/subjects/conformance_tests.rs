//! ADR#0055 conformance over the whole A2A subject inventory.
//!
//! Every subject type is constructed here and checked against the shared
//! validator. A new subject type that skips this list is the failure mode this
//! guards against, so [`published`] and [`patterns`] are the single place to
//! register one.

use trogon_nats::subject_conformance::{
    looks_like_request_id, validate_binding_version, validate_published_subject, validate_subject_pattern,
};

use super::{A2aStream, agents, subscriptions, tasks};
use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::constants::GATEWAY_INGRESS_METHOD_SUFFIXES;
use crate::gateway_ingress::compose_gateway_ingress_subject;
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

/// Deliberately not UUID-shaped. A task id is a durable resource identity, and
/// the request-id ratchet below must not confuse the two.
fn task() -> A2aTaskId {
    A2aTaskId::new("task_01").expect("test task id")
}

/// Every concrete subject a binding publishes to, paired with its type name.
fn published(p: &A2aPrefix) -> Vec<(&'static str, String)> {
    let a = agent();

    let mut out = vec![
        ("AgentCardSubject", agents::AgentCardSubject::new(p, &a).to_string()),
        ("MessageSendSubject", agents::MessageSendSubject::new(p, &a).to_string()),
        (
            "MessageStreamSubject",
            agents::MessageStreamSubject::new(p, &a).to_string(),
        ),
        ("PushSetSubject", agents::PushSetSubject::new(p, &a).to_string()),
        ("PushGetSubject", agents::PushGetSubject::new(p, &a).to_string()),
        ("PushListSubject", agents::PushListSubject::new(p, &a).to_string()),
        ("PushDeleteSubject", agents::PushDeleteSubject::new(p, &a).to_string()),
        ("TasksCancelSubject", agents::TasksCancelSubject::new(p, &a).to_string()),
        ("TasksGetSubject", agents::TasksGetSubject::new(p, &a).to_string()),
        ("TasksListSubject", agents::TasksListSubject::new(p, &a).to_string()),
        (
            "TasksResubscribeSubject",
            agents::TasksResubscribeSubject::new(p, &a).to_string(),
        ),
        (
            "TaskEventsSubject",
            tasks::TaskEventsSubject::new(p, &task()).to_string(),
        ),
    ];

    // Exhaustive by construction: the composer refuses any suffix outside this list.
    for suffix in GATEWAY_INGRESS_METHOD_SUFFIXES {
        let subject = compose_gateway_ingress_subject(p, &a, &suffix.join("."))
            .expect("GATEWAY_INGRESS_METHOD_SUFFIXES entry must compose");
        out.push(("gateway_ingress", subject));
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
    ];

    for stream in A2aStream::ALL {
        out.extend(stream.subject_patterns(p).into_iter().map(|s| ("A2aStream", s)));
    }
    out
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
