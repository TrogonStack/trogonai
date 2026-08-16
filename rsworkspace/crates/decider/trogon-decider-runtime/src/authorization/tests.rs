use std::sync::Arc;

use super::{
    AuthorizationDenied, CommandAuthorizer, CommandPrincipal, DirectedPrincipal, DirectedPrincipalError,
    PrincipalClaim, PrincipalClaimError, PrincipalClaims, PrincipalId, PrincipalIdError, PrincipalKind,
    UnauthorizedError, WithoutAuthorization,
};

struct Command;

/// Grants only to principals carrying the named claim.
struct RequireClaim(&'static str);

impl CommandAuthorizer<Command> for RequireClaim {
    fn authorize(&self, principal: &CommandPrincipal, _command: &Command) -> Result<(), AuthorizationDenied> {
        if principal.has_claim(self.0) {
            return Ok(());
        }
        Err(AuthorizationDenied::new(format!("missing claim {}", self.0)))
    }
}

/// An authorizer that deliberately permits anonymous execution.
struct AllowAnonymous;

impl CommandAuthorizer<Command> for AllowAnonymous {
    fn authorize(&self, _principal: &CommandPrincipal, _command: &Command) -> Result<(), AuthorizationDenied> {
        Ok(())
    }

    fn authorize_execution(
        &self,
        _principal: Option<&CommandPrincipal>,
        _command: &Command,
    ) -> Result<(), UnauthorizedError> {
        Ok(())
    }
}

fn principal_id(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("valid principal id")
}

fn claims(values: &[&str]) -> PrincipalClaims {
    values
        .iter()
        .map(|value| PrincipalClaim::new(*value).expect("valid claim"))
        .collect()
}

fn agent(id: &str, granted: &[&str]) -> CommandPrincipal {
    CommandPrincipal::new(PrincipalKind::Agent, principal_id(id)).with_claims(claims(granted))
}

#[test]
fn a_principal_id_rejects_the_empty_string() {
    assert_eq!(PrincipalId::new(""), Err(PrincipalIdError::Empty));
}

#[test]
fn a_principal_id_rejects_control_characters() {
    assert_eq!(
        PrincipalId::new("agent\u{0}1"),
        Err(PrincipalIdError::ContainsControlCharacter)
    );
}

#[test]
fn a_claim_rejects_the_empty_string() {
    assert_eq!(PrincipalClaim::new(""), Err(PrincipalClaimError::Empty));
}

#[test]
fn a_claim_rejects_control_characters() {
    assert_eq!(
        PrincipalClaim::new("orders\nwrite"),
        Err(PrincipalClaimError::ContainsControlCharacter)
    );
}

#[test]
fn a_directed_principal_distinguishes_absent_from_empty() {
    assert_eq!(DirectedPrincipal::new(""), Err(DirectedPrincipalError::Empty));

    let principal = CommandPrincipal::new(PrincipalKind::Agent, principal_id("agent-1"));
    assert_eq!(principal.directed_principal(), None);
}

#[test]
fn a_directed_principal_is_not_a_claim() {
    let directed = DirectedPrincipal::new("person-7").expect("valid directed principal");
    let principal =
        CommandPrincipal::new(PrincipalKind::Agent, principal_id("agent-1")).with_directed_principal(directed.clone());

    assert_eq!(principal.directed_principal(), Some(&directed));
    assert!(!principal.has_claim("person-7"));
    assert!(principal.claims().is_empty());
}

#[test]
fn granting_the_same_claim_twice_grants_it_once() {
    let granted = claims(&["orders.write", "orders.write", "orders.read"]);

    assert_eq!(granted.len(), 2);
    assert!(granted.contains("orders.write"));
    assert!(granted.contains("orders.read"));
    assert!(!granted.contains("orders.delete"));
}

#[test]
fn a_principal_renders_its_kind_and_id() {
    assert_eq!(agent("agent-1", &[]).to_string(), "agent:agent-1");
    assert_eq!(
        CommandPrincipal::new(PrincipalKind::Service, principal_id("scheduler")).to_string(),
        "service:scheduler"
    );
}

#[test]
fn an_unconfigured_execution_authorizes_anything() {
    assert_eq!(
        WithoutAuthorization.authorize_execution(None, &Command),
        Ok::<(), UnauthorizedError>(())
    );
    assert_eq!(
        WithoutAuthorization.authorize_execution(Some(&agent("agent-1", &[])), &Command),
        Ok::<(), UnauthorizedError>(())
    );
}

#[test]
fn a_configured_authorizer_refuses_an_execution_with_no_principal() {
    assert_eq!(
        RequireClaim("orders.write").authorize_execution(None, &Command),
        Err(UnauthorizedError::MissingPrincipal)
    );
}

#[test]
fn a_configured_authorizer_grants_a_principal_carrying_the_claim() {
    let principal = agent("agent-1", &["orders.write"]);

    assert_eq!(
        RequireClaim("orders.write").authorize_execution(Some(&principal), &Command),
        Ok(())
    );
}

#[test]
fn a_denial_carries_the_authorizer_reason() {
    let principal = agent("agent-1", &["orders.read"]);

    let error = RequireClaim("orders.write")
        .authorize_execution(Some(&principal), &Command)
        .expect_err("the principal does not carry the required claim");

    assert_eq!(
        error,
        UnauthorizedError::Denied(AuthorizationDenied::new("missing claim orders.write"))
    );
    assert_eq!(
        error.to_string(),
        "command denied for this principal: missing claim orders.write"
    );
}

/// Takes its authorizer by value, the way an execution's `Auth` parameter does,
/// so a borrowed or shared authorizer has to satisfy the trait in its own right.
fn authorize_anonymously<A>(authorizer: A) -> Result<(), UnauthorizedError>
where
    A: CommandAuthorizer<Command>,
{
    authorizer.authorize_execution(None, &Command)
}

#[test]
fn a_shared_authorizer_keeps_the_fail_closed_default() {
    let shared = Arc::new(RequireClaim("orders.write"));

    assert_eq!(
        authorize_anonymously(Arc::clone(&shared)),
        Err(UnauthorizedError::MissingPrincipal)
    );
    assert_eq!(
        authorize_anonymously(&shared),
        Err(UnauthorizedError::MissingPrincipal)
    );
}

#[test]
fn a_shared_authorizer_forwards_an_overridden_anonymous_policy() {
    let shared = Arc::new(AllowAnonymous);

    assert_eq!(authorize_anonymously(Arc::clone(&shared)), Ok(()));
    assert_eq!(authorize_anonymously(&AllowAnonymous), Ok(()));
}
