use trogon_decider_runtime::{CommandPrincipal, PrincipalId, PrincipalKind};
use trogon_nats::constants::MAX_NATS_TOKEN_LENGTH;

use super::{PrincipalPrefix, PrincipalPrefixError};
use crate::constants::{DECIDE_METHOD_NAME, DECIDER_SERVICE_NAME, DEFAULT_SUBJECT_PREFIX};
use crate::endpoint::{CommandEndpoint, SubjectPrefix};
use crate::tenant_position::TenantPosition;

fn prefix(value: &str) -> SubjectPrefix {
    SubjectPrefix::new(value).expect("a token")
}

fn id(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("an id")
}

#[test]
fn the_prefix_ends_in_the_principal_the_client_acts_for() {
    let principal = PrincipalPrefix::new(&prefix(DEFAULT_SUBJECT_PREFIX), PrincipalKind::Agent, &id("alice"))
        .expect("alice is a token");

    assert_eq!(principal.as_str(), "decider.agent.alice");
    assert_eq!(principal.to_string(), principal.as_str());
}

#[test]
fn a_tenant_leads_the_prefix_ahead_of_the_namespace() {
    let principal =
        PrincipalPrefix::new(&prefix("acme.decider"), PrincipalKind::Person, &id("bob")).expect("bob is a token");

    assert_eq!(principal.as_str(), "acme.decider.person.bob");
}

#[test]
fn the_host_reads_back_the_principal_the_prefix_names() {
    let endpoint = CommandEndpoint::new(prefix(DEFAULT_SUBJECT_PREFIX), TenantPosition::Absent).expect("an endpoint");
    let principal = PrincipalPrefix::new(endpoint.prefix(), PrincipalKind::Service, &id("scheduler"))
        .expect("scheduler is a token");

    assert_eq!(
        endpoint
            .derive_principal(&format!("{principal}.{DECIDER_SERVICE_NAME}.{DECIDE_METHOD_NAME}"))
            .expect("the subject names a principal"),
        CommandPrincipal::new(PrincipalKind::Service, id("scheduler")),
        "a client and the host that disagree on where the principal sits would authorize every command as someone else"
    );
}

#[test]
fn an_id_that_is_not_one_subject_token_is_refused() {
    for unsafe_id in ["alice.smith", "a*b", "a>b", "has space"] {
        assert!(
            matches!(
                PrincipalPrefix::new(&prefix(DEFAULT_SUBJECT_PREFIX), PrincipalKind::Agent, &id(unsafe_id)),
                Err(PrincipalPrefixError::Principal(_))
            ),
            "{unsafe_id:?} would split or widen the subject, naming a different principal or many"
        );
    }
}

#[test]
fn a_prefix_too_long_to_carry_the_principal_is_refused() {
    let long = "p".repeat(MAX_NATS_TOKEN_LENGTH);

    assert!(matches!(
        PrincipalPrefix::new(&prefix(&long), PrincipalKind::Agent, &id("alice")),
        Err(PrincipalPrefixError::Prefix(_))
    ));
}
