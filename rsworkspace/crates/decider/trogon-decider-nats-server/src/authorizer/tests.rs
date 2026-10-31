use trogon_decider_runtime::{CommandAuthorizer as _, CommandPrincipal, PrincipalId, PrincipalKind, UnauthorizedError};
use trogon_decider_wit::host::CommandEnvelope;

use super::AdmitAnyPrincipal;

fn command() -> CommandEnvelope {
    CommandEnvelope {
        type_: "trogonai.scheduler.schedules.v1.CreateSchedule".to_owned(),
        payload: Vec::new(),
    }
}

#[test]
fn the_default_authorizer_admits_a_principal_the_subject_named() {
    let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new("alice").expect("an id"));

    assert_eq!(
        AdmitAnyPrincipal.authorize_execution(Some(&principal), &command()),
        Ok(()),
        "a host with no policy configured still has to serve the principals its subjects name"
    );
}

#[test]
fn the_default_authorizer_refuses_an_execution_with_no_principal() {
    assert_eq!(
        AdmitAnyPrincipal.authorize_execution(None, &command()),
        Err(UnauthorizedError::MissingPrincipal),
        "admitting any principal is not admitting nobody, so an execution that lost its principal is refused"
    );
}
