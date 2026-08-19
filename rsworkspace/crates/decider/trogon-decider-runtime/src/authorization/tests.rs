use std::sync::Arc;

use super::{
    AuthorizationDeniedError, CommandAuthorizer, CommandPrincipal, DirectedPrincipal, DirectedPrincipalError,
    PrincipalClaim, PrincipalClaimError, PrincipalClaims, PrincipalId, PrincipalIdError, PrincipalKind,
    UnauthorizedError, WithoutAuthorization,
};

struct Command;

/// Grants only to principals carrying the named claim.
struct RequireClaim(&'static str);

impl CommandAuthorizer<Command> for RequireClaim {
    fn authorize(&self, principal: &CommandPrincipal, _command: &Command) -> Result<(), AuthorizationDeniedError> {
        if principal.has_claim(self.0) {
            return Ok(());
        }
        Err(AuthorizationDeniedError::new(format!("missing claim {}", self.0)))
    }
}

/// An authorizer that deliberately permits anonymous execution.
struct AllowAnonymous;

impl CommandAuthorizer<Command> for AllowAnonymous {
    fn authorize(&self, _principal: &CommandPrincipal, _command: &Command) -> Result<(), AuthorizationDeniedError> {
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
fn every_way_of_naming_a_principal_id_names_the_same_one() {
    let expected = principal_id("agent-1");

    assert_eq!("agent-1".parse::<PrincipalId>(), Ok(expected.clone()));
    assert_eq!(PrincipalId::try_from("agent-1"), Ok(expected.clone()));
    assert_eq!(PrincipalId::try_from("agent-1".to_owned()), Ok(expected.clone()));
    assert_eq!(
        expected.to_string().parse::<PrincipalId>(),
        Ok(expected.clone()),
        "an identifier that does not reparse from its own rendered form cannot be carried through a log or a wire hop"
    );
    assert_eq!(expected.as_str(), "agent-1");
    assert_eq!(AsRef::<str>::as_ref(&expected), "agent-1");
}

#[test]
fn every_way_of_naming_a_claim_names_the_same_one() {
    let expected = PrincipalClaim::new("orders.write").expect("valid claim");

    assert_eq!("orders.write".parse::<PrincipalClaim>(), Ok(expected.clone()));
    assert_eq!(PrincipalClaim::try_from("orders.write"), Ok(expected.clone()));
    assert_eq!(
        PrincipalClaim::try_from("orders.write".to_owned()),
        Ok(expected.clone())
    );
    assert_eq!(expected.to_string().parse::<PrincipalClaim>(), Ok(expected.clone()));
    assert_eq!(expected.as_str(), "orders.write");
    assert_eq!(AsRef::<str>::as_ref(&expected), "orders.write");
}

#[test]
fn every_way_of_naming_a_directed_principal_names_the_same_one() {
    let expected = DirectedPrincipal::new("person-7").expect("valid directed principal");

    assert_eq!("person-7".parse::<DirectedPrincipal>(), Ok(expected.clone()));
    assert_eq!(DirectedPrincipal::try_from("person-7"), Ok(expected.clone()));
    assert_eq!(DirectedPrincipal::try_from("person-7".to_owned()), Ok(expected.clone()));
    assert_eq!(expected.to_string().parse::<DirectedPrincipal>(), Ok(expected.clone()));
    assert_eq!(expected.as_str(), "person-7");
    assert_eq!(AsRef::<str>::as_ref(&expected), "person-7");
}

#[test]
fn a_directed_principal_rejects_control_characters() {
    assert_eq!(
        DirectedPrincipal::new("person\u{7}7"),
        Err(DirectedPrincipalError::ContainsControlCharacter)
    );
}

#[test]
fn every_kind_of_actor_has_a_wire_name_that_is_also_what_it_prints() {
    let kinds = [
        (PrincipalKind::Agent, "agent"),
        (PrincipalKind::Person, "person"),
        (PrincipalKind::Service, "service"),
    ];

    for (kind, name) in kinds {
        assert_eq!(kind.as_str(), name);
        assert_eq!(
            kind.to_string(),
            name,
            "a kind that printed differently from its wire name would make an audit line disagree with the token it came from"
        );
    }
}

#[test]
fn a_claim_set_starts_empty_and_reports_what_was_added() {
    let mut granted = PrincipalClaims::empty();

    assert!(granted.is_empty());
    assert_eq!(granted.len(), 0);
    assert!(granted.insert(PrincipalClaim::new("orders.write").expect("valid claim")));
    assert!(
        !granted.insert(PrincipalClaim::new("orders.write").expect("valid claim")),
        "an issuing boundary that sent a claim twice has not granted anything the second time"
    );
    assert_eq!(granted.len(), 1);
}

#[test]
fn a_claim_set_iterates_in_a_stable_order_however_it_is_consumed() {
    let granted = claims(&["orders.write", "billing.read", "orders.read"]);
    let expected = ["billing.read", "orders.read", "orders.write"];

    assert_eq!(
        granted.iter().map(PrincipalClaim::as_str).collect::<Vec<_>>(),
        expected,
        "an authorizer logging its input should not see the order the issuing boundary happened to emit"
    );
    assert_eq!((&granted).into_iter().count(), 3);
    assert_eq!(
        granted
            .into_iter()
            .map(|claim| claim.as_str().to_owned())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn a_principal_reports_the_actor_it_was_built_for() {
    let principal = agent("agent-1", &["orders.write"]);

    assert_eq!(principal.kind(), PrincipalKind::Agent);
    assert_eq!(principal.id(), &principal_id("agent-1"));
    assert_eq!(principal.claims(), &claims(&["orders.write"]));
}

#[test]
fn a_directed_principal_can_be_cleared_as_well_as_set() {
    let principal = CommandPrincipal::new(PrincipalKind::Agent, principal_id("agent-1")).with_directed_principal(None);

    assert_eq!(
        principal.directed_principal(),
        None,
        "a boundary that saw no directed principal has to be able to say so without inventing one"
    );
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
        UnauthorizedError::Denied(AuthorizationDeniedError::new("missing claim orders.write"))
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
    assert_eq!(authorize_anonymously(&shared), Err(UnauthorizedError::MissingPrincipal));
}

#[test]
fn a_shared_authorizer_forwards_an_overridden_anonymous_policy() {
    let shared = Arc::new(AllowAnonymous);

    assert_eq!(authorize_anonymously(Arc::clone(&shared)), Ok(()));
    assert_eq!(authorize_anonymously(&AllowAnonymous), Ok(()));
}
