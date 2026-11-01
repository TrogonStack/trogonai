use trogon_identity_types::{PrincipalId, PrincipalKind, SubjectPrincipal};

use super::*;

fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_credential(PrincipalKind::Service, &PrincipalId::new("caller-1").unwrap()).unwrap()
}

#[test]
fn caller_request_patterns_keep_the_principal_position() {
    let prefix = A2aPrefix::new("acme.a2a").unwrap();
    let caller = caller();
    assert_eq!(
        CallerAgentRequestsSubject::new(&prefix, &caller).to_string(),
        "acme.a2a.v1.agents.*.service.caller-1.>"
    );
    assert_eq!(
        CallerGatewayRequestsSubject::new(&prefix, &caller).to_string(),
        "acme.a2a.v1.gateway.*.service.caller-1.>"
    );
}
