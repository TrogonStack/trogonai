use trogon_decider_runtime::{CommandPrincipal, PrincipalId, PrincipalKind};
use trogon_identity_types::SubjectPrincipalId;
use trogon_nats::constants::MAX_NATS_TOKEN_LENGTH;
use trogon_nats::{MAX_SUBJECT_TOKENS, SubjectViolationError};

use super::{CommandEndpoint, CommandEndpointError, CommandSubjectError, SubjectPrefix};
use crate::constants::DEFAULT_SUBJECT_PREFIX;
use crate::tenant_position::TenantPosition;

fn endpoint(prefix: &str) -> CommandEndpoint {
    CommandEndpoint::new(SubjectPrefix::new(prefix).expect("a token"), TenantPosition::Absent)
        .expect("a conformant subject")
}

fn tenant_endpoint(prefix: &str) -> CommandEndpoint {
    CommandEndpoint::new(SubjectPrefix::new(prefix).expect("a token"), TenantPosition::Wildcard)
        .expect("a conformant subject")
}

fn principal(kind: PrincipalKind, id: &str) -> CommandPrincipal {
    CommandPrincipal::new(kind, PrincipalId::new(id).expect("an id"))
}

fn dotted_prefix(tokens: usize) -> SubjectPrefix {
    SubjectPrefix::new(core::iter::repeat_n("seg", tokens).collect::<Vec<_>>().join("."))
        .expect("a short dotted prefix is a token")
}

#[test]
fn the_endpoint_subscribes_with_wildcards_where_the_principal_sits() {
    assert_eq!(
        endpoint(DEFAULT_SUBJECT_PREFIX).subscription(),
        "decider.*.*.DeciderService.Decide",
        "a caller's prefix ends in its principal, so the host has to match every principal between its namespace and the method"
    );
}

#[test]
fn a_configured_prefix_moves_the_endpoint_without_renaming_it() {
    let moved = endpoint("acme.decider");

    assert_eq!(moved.subscription(), "acme.decider.*.*.DeciderService.Decide");
    assert_eq!(moved.prefix().as_str(), "acme.decider");
    assert_eq!(moved.tenant(), TenantPosition::Absent);
}

#[test]
fn a_host_serving_several_tenants_subscribes_at_the_tenant_position_too() {
    let shared = tenant_endpoint(DEFAULT_SUBJECT_PREFIX);

    assert_eq!(shared.subscription(), "*.decider.*.*.DeciderService.Decide");
    assert_eq!(shared.tenant(), TenantPosition::Wildcard);
}

#[test]
fn a_prefix_that_is_not_a_nats_token_is_refused() {
    assert!(SubjectPrefix::new("has space").is_err());
    assert!(SubjectPrefix::new("wild*card").is_err());
}

#[test]
fn a_prefix_whose_subject_breaks_the_subject_limits_is_refused_at_startup() {
    // The endpoint appends four tokens, so this prefix is legal on its own and
    // only the appended tokens put the subject one over the budget.
    let error = CommandEndpoint::new(dotted_prefix(MAX_SUBJECT_TOKENS - 3), TenantPosition::Absent)
        .expect_err("the joined subject is over the limit");

    assert!(
        matches!(
            error,
            CommandEndpointError::Subject {
                source: SubjectViolationError::TooManyTokens { .. },
                ..
            }
        ),
        "a host that cannot legally publish its own endpoint subject can never receive a command, so it has to fail before it claims to be serving: {error}"
    );
}

#[test]
fn the_tenant_token_counts_against_the_subject_budget() {
    let prefix = dotted_prefix(MAX_SUBJECT_TOKENS - 4);

    assert!(CommandEndpoint::new(prefix.clone(), TenantPosition::Absent).is_ok());
    assert!(
        matches!(
            CommandEndpoint::new(prefix, TenantPosition::Wildcard),
            Err(CommandEndpointError::Subject {
                source: SubjectViolationError::TooManyTokens { .. },
                ..
            })
        ),
        "every subject a shared host receives carries the tenant token, so the budget has to account for it"
    );
}

#[test]
fn every_principal_kind_is_read_from_the_subject() {
    let endpoint = endpoint(DEFAULT_SUBJECT_PREFIX);

    for (subject, expected) in [
        (
            "decider.agent.alice.DeciderService.Decide",
            principal(PrincipalKind::Agent, "alice"),
        ),
        (
            "decider.person.bob.DeciderService.Decide",
            principal(PrincipalKind::Person, "bob"),
        ),
        (
            "decider.service.scheduler.DeciderService.Decide",
            principal(PrincipalKind::Service, "scheduler"),
        ),
    ] {
        assert_eq!(
            endpoint
                .derive_principal(subject)
                .expect("the subject names a principal"),
            expected,
            "{subject}"
        );
    }
}

#[test]
fn the_principal_carries_nothing_the_subject_does_not() {
    let derived = endpoint(DEFAULT_SUBJECT_PREFIX)
        .derive_principal("decider.agent.alice.DeciderService.Decide")
        .expect("the subject names a principal");

    assert!(derived.claims().is_empty(), "the subject grants no claims");
    assert!(
        derived.directed_principal().is_none(),
        "the subject has no position for a directed principal"
    );
}

#[test]
fn a_shared_host_reads_the_principal_after_the_tenant_token() {
    assert_eq!(
        tenant_endpoint(DEFAULT_SUBJECT_PREFIX)
            .derive_principal("acme.decider.agent.alice.DeciderService.Decide")
            .expect("the subject names a principal"),
        principal(PrincipalKind::Agent, "alice")
    );
}

#[test]
fn a_kind_token_that_is_no_principal_kind_is_refused() {
    assert!(matches!(
        endpoint(DEFAULT_SUBJECT_PREFIX).derive_principal("decider.robot.alice.DeciderService.Decide"),
        Err(CommandSubjectError::Principal { .. })
    ));
}

#[test]
fn an_id_token_that_is_no_principal_id_is_refused() {
    let endpoint = endpoint(DEFAULT_SUBJECT_PREFIX);

    for subject in [
        "decider.agent.a*b.DeciderService.Decide",
        "decider.agent.a>b.DeciderService.Decide",
        "decider.agent.a\u{1}b.DeciderService.Decide",
        "decider.agent.\u{e5}lice.DeciderService.Decide",
    ] {
        assert!(
            matches!(
                endpoint.derive_principal(subject),
                Err(CommandSubjectError::Principal { .. })
            ),
            "{subject:?}"
        );
    }
}

#[test]
fn an_id_token_no_edge_would_put_in_a_subject_is_refused() {
    let endpoint = endpoint(DEFAULT_SUBJECT_PREFIX);
    let one_whole_token = "a".repeat(MAX_NATS_TOKEN_LENGTH);

    for id in [one_whole_token.as_str(), "~", "~not-a-digest"] {
        assert!(
            matches!(
                endpoint.derive_principal(&format!("decider.agent.{id}.DeciderService.Decide")),
                Err(CommandSubjectError::Principal { .. })
            ),
            "{id:?} is not a token ADR#0082's mapping produces, so no edge put it in a subject for a verified principal"
        );
    }
}

#[test]
fn a_hashed_id_token_is_read_as_the_principal_it_carries() {
    let hashed = SubjectPrincipalId::from(&PrincipalId::new("alice.smith").expect("an id"));

    assert!(hashed.is_hashed());
    assert_eq!(
        endpoint(DEFAULT_SUBJECT_PREFIX)
            .derive_principal(&format!("decider.agent.{hashed}.DeciderService.Decide"))
            .expect("an edge carries an id that is not one token in its hashed form"),
        principal(PrincipalKind::Agent, hashed.as_str()),
        "the host derives the token itself, so an authorizer sees the hashed id"
    );
}

#[test]
fn a_subject_that_is_not_the_endpoint_s_names_no_principal() {
    let endpoint = endpoint(DEFAULT_SUBJECT_PREFIX);

    for subject in [
        "decider.DeciderService.Decide",
        "decider.agent.DeciderService.Decide",
        "other.agent.alice.DeciderService.Decide",
        "decider.agent.alice.DeciderService.Describe",
        "decider.agent.alice.OtherService.Decide",
        "decider.agent.alice.DeciderService.Decide.extra",
        "acme.decider.agent.alice.DeciderService.Decide",
    ] {
        assert!(
            matches!(
                endpoint.derive_principal(subject),
                Err(CommandSubjectError::OutsideEndpoint { .. })
            ),
            "{subject:?}"
        );
    }
}

#[test]
fn a_shared_host_refuses_a_subject_without_its_tenant_token() {
    let shared = tenant_endpoint(DEFAULT_SUBJECT_PREFIX);

    for subject in [
        "decider.agent.alice.DeciderService.Decide",
        ".decider.agent.alice.DeciderService.Decide",
    ] {
        assert!(
            matches!(
                shared.derive_principal(subject),
                Err(CommandSubjectError::OutsideEndpoint { .. })
            ),
            "{subject:?}"
        );
    }
}
