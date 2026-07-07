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
    let err = PersonServerError::MissionNotFound(crate::mission::MissionId("abc".to_string()));
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
