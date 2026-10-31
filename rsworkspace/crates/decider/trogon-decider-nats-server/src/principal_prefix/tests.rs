use trogon_decider_runtime::{CommandPrincipal, PrincipalId, PrincipalKind};
use trogon_identity_types::{SubjectPrincipalError, SubjectPrincipalIdError};
use trogon_nats::constants::MAX_NATS_TOKEN_LENGTH;
use trogon_nats::{MAX_SUBJECT_TOKENS, SubjectViolationError};

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

fn id_token(principal: &PrincipalPrefix) -> &str {
    principal
        .as_str()
        .rsplit_once('.')
        .map(|(_, token)| token)
        .expect("a prefix ends in the id token")
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
fn an_id_that_is_not_one_subject_token_is_carried_hashed() {
    let endpoint = CommandEndpoint::new(prefix(DEFAULT_SUBJECT_PREFIX), TenantPosition::Absent).expect("an endpoint");

    for unsafe_id in ["alice.smith", "a*b", "a>b", "has space"] {
        let principal = PrincipalPrefix::new(endpoint.prefix(), PrincipalKind::Agent, &id(unsafe_id))
            .expect("an id that is not one token is hashed rather than refused");
        let token = id_token(&principal);

        assert!(token.starts_with('~'), "{unsafe_id:?} is carried in its hashed form");
        assert_eq!(
            principal.as_str(),
            format!("decider.agent.{token}"),
            "{unsafe_id:?} carried as is would split or widen the subject, naming a different principal or many"
        );
        assert_eq!(
            endpoint
                .derive_principal(&format!("{principal}.{DECIDER_SERVICE_NAME}.{DECIDE_METHOD_NAME}"))
                .expect("the subject names a principal"),
            CommandPrincipal::new(PrincipalKind::Agent, id(token)),
            "the host derives the hashed token, so that is the id an authorizer sees for {unsafe_id:?}"
        );
    }
}

#[test]
fn an_id_that_already_has_the_hashed_form_is_refused() {
    let namespace = prefix(DEFAULT_SUBJECT_PREFIX);
    let hashed =
        PrincipalPrefix::new(&namespace, PrincipalKind::Agent, &id("alice.smith")).expect("alice.smith hashes");

    assert!(
        matches!(
            PrincipalPrefix::new(&namespace, PrincipalKind::Agent, &id(id_token(&hashed))),
            Err(PrincipalPrefixError::Principal(SubjectPrincipalError::Id(
                SubjectPrincipalIdError::AlreadyHashed
            )))
        ),
        "a credential whose id is already a digest would act as whichever principal hashes to it"
    );
}

#[test]
fn a_prefix_longer_than_one_token_reaches_the_host_while_each_token_fits() {
    let endpoint = CommandEndpoint::new(prefix(&"p".repeat(MAX_NATS_TOKEN_LENGTH)), TenantPosition::Absent)
        .expect("the longest namespace token leaves room for the endpoint subject");
    let principal = PrincipalPrefix::new(endpoint.prefix(), PrincipalKind::Agent, &id("alice"))
        .expect("the token limit bounds each token, not the prefix they form");

    assert!(principal.as_str().len() > MAX_NATS_TOKEN_LENGTH);
    assert_eq!(
        endpoint
            .derive_principal(&format!("{principal}.{DECIDER_SERVICE_NAME}.{DECIDE_METHOD_NAME}"))
            .expect("the subject names a principal"),
        CommandPrincipal::new(PrincipalKind::Agent, id("alice")),
        "a client refused a prefix the host serves could never reach that host"
    );
}

#[test]
fn an_id_longer_than_one_token_is_carried_hashed() {
    let endpoint = CommandEndpoint::new(prefix(DEFAULT_SUBJECT_PREFIX), TenantPosition::Absent).expect("an endpoint");
    let principal = PrincipalPrefix::new(
        endpoint.prefix(),
        PrincipalKind::Agent,
        &id(&"a".repeat(MAX_NATS_TOKEN_LENGTH + 1)),
    )
    .expect("an id too long for one token is hashed rather than refused");
    let token = id_token(&principal);

    assert!(token.starts_with('~') && token.len() <= MAX_NATS_TOKEN_LENGTH);
    assert_eq!(
        endpoint
            .derive_principal(&format!("{principal}.{DECIDER_SERVICE_NAME}.{DECIDE_METHOD_NAME}"))
            .expect("the subject names a principal"),
        CommandPrincipal::new(PrincipalKind::Agent, id(token)),
        "a client refused for the length of its id could never reach the host for that principal"
    );
}

#[test]
fn a_subject_over_the_token_budget_is_refused() {
    let namespace = prefix(&["t"; MAX_SUBJECT_TOKENS - 3].join("."));

    assert!(matches!(
        PrincipalPrefix::new(&namespace, PrincipalKind::Agent, &id("alice")),
        Err(PrincipalPrefixError::Subject {
            source: SubjectViolationError::TooManyTokens { .. },
            ..
        })
    ));
}
