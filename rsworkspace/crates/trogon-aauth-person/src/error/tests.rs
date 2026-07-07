use trogon_aauth_verify::TokenError;
use trogon_identity_types::aauth::error::{InteractionEndpointError, PollingError};

use super::*;

#[test]
fn agent_token_error_maps_to_invalid_agent_token() {
    let err = PersonServerError::Verification(RequestVerificationError::AgentToken(TokenError::BadHeader));
    assert_eq!(err.token_endpoint_code(), TokenEndpointError::InvalidAgentToken);
    assert_eq!(err.http_status(), 400);
}

#[test]
fn expired_resource_token_maps_to_expired_resource_token_code() {
    let err = PersonServerError::Verification(RequestVerificationError::ResourceToken(TokenError::Expired));
    assert_eq!(err.token_endpoint_code(), TokenEndpointError::ExpiredResourceToken);
    assert_eq!(err.http_status(), 400);
}

#[test]
fn user_unreachable_maps_to_403() {
    let err = PersonServerError::UserUnreachable;
    assert_eq!(err.token_endpoint_code(), TokenEndpointError::UserUnreachable);
    assert_eq!(err.http_status(), 403);
}

#[test]
fn interaction_relay_user_unreachable_maps_like_top_level_user_unreachable() {
    let err = PersonServerError::Interaction(InteractionRelayError::UserUnreachable);
    assert_eq!(err.token_endpoint_code(), TokenEndpointError::UserUnreachable);
    assert_eq!(err.http_status(), 403);
}

#[test]
fn denied_maps_to_denied_wire_code_and_403() {
    let err = PersonServerError::Denied("not within mission scope".to_string());
    assert_eq!(err.wire_code(), "denied");
    assert_eq!(err.http_status(), 403);
}

#[test]
fn server_error_fallback_maps_to_500() {
    let err = PersonServerError::MissionNotFound(crate::mission::MissionId::from_s256("abc"));
    assert_eq!(err.token_endpoint_code(), TokenEndpointError::ServerError);
    assert_eq!(err.http_status(), 500);
    assert_eq!(err.wire_code(), "server_error");
}

#[test]
fn polling_error_status_matches_draft_table() {
    assert_eq!(polling_error_status(&PollingError::Denied), 403);
    assert_eq!(polling_error_status(&PollingError::Abandoned), 403);
    assert_eq!(polling_error_status(&PollingError::Expired), 408);
    assert_eq!(polling_error_status(&PollingError::InvalidCode), 410);
    assert_eq!(polling_error_status(&PollingError::SlowDown), 429);
    assert_eq!(polling_error_status(&PollingError::ServerError), 500);
}

#[test]
fn interaction_endpoint_error_status_matches_draft_table() {
    assert_eq!(
        interaction_endpoint_error_status(&InteractionEndpointError::InteractionUnavailable),
        424
    );
}

#[test]
fn wire_code_matches_token_endpoint_error_serde_rename() {
    let err = PersonServerError::Verification(RequestVerificationError::AgentToken(TokenError::BadHeader));
    assert_eq!(err.wire_code(), "invalid_agent_token");
}

#[test]
fn client_detail_does_not_leak_verification_comparison_values() {
    let err = PersonServerError::Verification(RequestVerificationError::ResourceTokenWrongAudience {
        expected: "https://ps.internal.example".to_string(),
        actual: "https://evil.example".to_string(),
    });
    let detail = err.client_detail().expect("generic detail");
    assert!(!detail.contains("ps.internal.example"));
    assert!(!detail.contains("evil.example"));
    assert_eq!(detail, "request verification failed");
}

#[test]
fn client_detail_is_omitted_for_internal_failures() {
    let err = PersonServerError::Store(crate::store::StoreError("dsn=postgres://user:pw@db".to_string()));
    assert!(err.client_detail().is_none());

    let policy = PersonServerError::Policy(Box::new(crate::store::StoreError("internal".to_string())));
    assert!(policy.client_detail().is_none());
}

#[test]
fn client_detail_preserves_denied_reason() {
    let err = PersonServerError::Denied("not within mission scope".to_string());
    assert_eq!(err.client_detail().as_deref(), Some("denied: not within mission scope"));
}

#[test]
fn client_detail_forwards_pending_not_found_display() {
    let pending_id = crate::pending::PendingId("pending-123".to_string());
    let err = PersonServerError::Pending(PendingRequestError::NotFound(pending_id));
    assert_eq!(
        err.client_detail().as_deref(),
        Some("no pending request for id pending-123")
    );
}

#[test]
fn client_detail_forwards_pending_gone_display() {
    let pending_id = crate::pending::PendingId("pending-456".to_string());
    let err = PersonServerError::Pending(PendingRequestError::Gone(pending_id));
    assert_eq!(
        err.client_detail().as_deref(),
        Some("pending request pending-456 is gone (canceled or already resolved)")
    );
}

#[test]
fn client_detail_forwards_clarification_limit_exceeded_display() {
    let pending_id = crate::pending::PendingId("pending-789".to_string());
    let err = PersonServerError::Pending(PendingRequestError::ClarificationLimitExceeded(pending_id, 3));
    assert_eq!(
        err.client_detail().as_deref(),
        Some("clarification round limit (3) exceeded for pending request pending-789")
    );
}

#[test]
fn client_detail_forwards_updated_request_identity_mismatch_display() {
    let err = PersonServerError::Pending(PendingRequestError::UpdatedRequestIdentityMismatch);
    assert_eq!(
        err.client_detail().as_deref(),
        Some("updated_request resource_token does not match original iss/agent/agent_jkt")
    );
}

#[test]
fn resource_token_subagent_key_mismatch_maps_to_invalid_resource_token() {
    let err = PersonServerError::Verification(RequestVerificationError::ResourceTokenSubagentKeyMismatch);
    assert_eq!(err.token_endpoint_code(), TokenEndpointError::InvalidResourceToken);
    assert_eq!(err.http_status(), 400);
}

#[test]
fn upstream_token_error_maps_to_invalid_request() {
    let err = PersonServerError::Verification(RequestVerificationError::UpstreamToken(TokenError::BadHeader));
    assert_eq!(err.token_endpoint_code(), TokenEndpointError::InvalidRequest);
    assert_eq!(err.http_status(), 400);
}

#[test]
fn client_detail_forwards_mission_validation_display() {
    let err = PersonServerError::MissionValidation(crate::mission::MissionValidationError::EmptyField("approver"));
    assert_eq!(
        err.client_detail().as_deref(),
        Some("mission blob field approver must not be empty")
    );
}
