use trogon_identity_types::aauth::person_server::ClarificationRequired;
use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims};

use super::*;

fn agent_claims() -> AgentClaims {
    AgentClaims {
        iss: "https://ap.example".to_string(),
        sub: "aauth:assistant@agent.example".to_string(),
        jti: "agent-jti".to_string(),
        iat: 0,
        exp: 1000,
        dwk: "aauth-agent.json".to_string(),
        cnf: Cnf {
            jwk: serde_json::json!({"kty": "EC"}),
        },
        ps: None,
    }
}

fn resource_claims() -> ResourceClaims {
    ResourceClaims {
        iss: "https://calendar.example".to_string(),
        aud: "https://ps.example".to_string(),
        jti: "resource-jti".to_string(),
        iat: 0,
        exp: 1000,
        dwk: "aauth-resource.json".to_string(),
        agent: "aauth:assistant@agent.example".to_string(),
        agent_jkt: "jkt".to_string(),
        scope: "calendar.read".to_string(),
        mission: None,
    }
}

fn clarification() -> ClarificationRequired {
    ClarificationRequired {
        status: "pending".to_string(),
        clarification: "Why do you need write access?".to_string(),
        timeout: None,
        options: None,
    }
}

#[test]
fn correlation_key_is_stable_for_same_agent_and_resource() {
    let a = PendingRequest::correlation_key_for(&agent_claims(), &resource_claims());
    let b = PendingRequest::correlation_key_for(&agent_claims(), &resource_claims());
    assert_eq!(a, b);
}

#[test]
fn correlation_key_differs_for_different_resource_jti() {
    let mut other_resource = resource_claims();
    other_resource.jti = "different-jti".to_string();
    let a = PendingRequest::correlation_key_for(&agent_claims(), &resource_claims());
    let b = PendingRequest::correlation_key_for(&agent_claims(), &other_resource);
    assert_ne!(a, b);
}

#[test]
fn new_pending_request_starts_in_pending_phase() {
    let pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    assert_eq!(pending.phase, PendingPhase::Pending);
    assert_eq!(pending.clarification_round, 0);
    assert!(!pending.is_terminal());
}

#[test]
fn begin_clarification_increments_round_and_sets_phase() {
    let mut pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    pending.begin_clarification(clarification()).unwrap();
    assert_eq!(pending.clarification_round, 1);
    assert!(matches!(pending.phase, PendingPhase::AwaitingClarification { .. }));
}

#[test]
fn clarification_rounds_are_capped_at_max() {
    let mut pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    for _ in 0..MAX_CLARIFICATION_ROUNDS {
        pending.begin_clarification(clarification()).unwrap();
    }
    let err = pending.begin_clarification(clarification()).unwrap_err();
    assert!(matches!(
        err,
        PendingRequestError::ClarificationLimitExceeded(_, MAX_CLARIFICATION_ROUNDS)
    ));
}

#[test]
fn begin_interaction_sets_interacting_phase() {
    let mut pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    pending.begin_interaction();
    assert_eq!(pending.phase, PendingPhase::Interacting);
}

#[test]
fn begin_resource_interaction_carries_url_and_code() {
    let mut pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    pending.begin_resource_interaction("https://booking.example/confirm".to_string(), "X7K2-M9P4".to_string());
    match pending.phase {
        PendingPhase::AwaitingResourceInteraction { url, code } => {
            assert_eq!(url, "https://booking.example/confirm");
            assert_eq!(code, "X7K2-M9P4");
        }
        other => panic!("expected AwaitingResourceInteraction, got {other:?}"),
    }
}

#[test]
fn grant_deny_cancel_are_terminal() {
    let mut granted = PendingRequest::new(agent_claims(), resource_claims(), None);
    granted.grant("token".to_string(), 3600);
    assert!(granted.is_terminal());

    let mut denied = PendingRequest::new(agent_claims(), resource_claims(), None);
    denied.deny("no".to_string());
    assert!(denied.is_terminal());

    let mut canceled = PendingRequest::new(agent_claims(), resource_claims(), None);
    canceled.cancel();
    assert!(canceled.is_terminal());
}

#[test]
fn non_terminal_phases_are_not_terminal() {
    let mut interacting = PendingRequest::new(agent_claims(), resource_claims(), None);
    interacting.begin_interaction();
    assert!(!interacting.is_terminal());

    let mut approval = PendingRequest::new(agent_claims(), resource_claims(), None);
    approval.begin_approval_pending();
    assert!(!approval.is_terminal());
}

#[test]
fn apply_updated_resource_accepts_matching_identity() {
    let mut pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    let mut updated = resource_claims();
    updated.scope = "calendar.readonly".to_string();
    pending.apply_updated_resource(updated.clone()).unwrap();
    assert_eq!(pending.resource.scope, "calendar.readonly");
    assert_eq!(pending.phase, PendingPhase::Pending);
}

#[test]
fn apply_updated_resource_rejects_identity_change() {
    let mut pending = PendingRequest::new(agent_claims(), resource_claims(), None);
    let mut updated = resource_claims();
    updated.agent_jkt = "different-jkt".to_string();
    let err = pending.apply_updated_resource(updated).unwrap_err();
    assert!(matches!(err, PendingRequestError::UpdatedRequestIdentityMismatch));
}

#[test]
fn pending_id_generate_produces_unique_ids() {
    let a = PendingId::generate();
    let b = PendingId::generate();
    assert_ne!(a, b);
}
