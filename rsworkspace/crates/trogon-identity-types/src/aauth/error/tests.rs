use super::*;

#[test]
fn error_response_minimal_round_trip() {
    let err = ErrorResponse::new("expired_resource_token")
        .with_detail("The resource token expired; obtain a new resource token from the resource and retry.");
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["error"], "expired_resource_token");
    assert!(json.get("type").is_none());
    let back: ErrorResponse = serde_json::from_value(json).unwrap();
    assert_eq!(back, err);
}

#[test]
fn error_response_matches_denied_example() {
    let raw = serde_json::json!({
        "error": "denied",
        "detail": "The user declined the request."
    });
    let err: ErrorResponse = serde_json::from_value(raw).unwrap();
    assert_eq!(err.error, "denied");
    assert_eq!(err.detail.as_deref(), Some("The user declined the request."));
}

#[test]
fn token_endpoint_error_serializes_snake_case() {
    assert_eq!(
        serde_json::to_value(&TokenEndpointError::ExpiredResourceToken).unwrap(),
        "expired_resource_token"
    );
    assert_eq!(
        serde_json::to_value(&TokenEndpointError::UserUnreachable).unwrap(),
        "user_unreachable"
    );
}

#[test]
fn polling_error_serializes_snake_case() {
    assert_eq!(
        serde_json::to_value(&PollingError::InvalidCode).unwrap(),
        "invalid_code"
    );
    assert_eq!(serde_json::to_value(&PollingError::SlowDown).unwrap(), "slow_down");
}

#[test]
fn interaction_callback_error_serializes_snake_case() {
    assert_eq!(
        serde_json::to_value(&InteractionCallbackError::AccessDenied).unwrap(),
        "access_denied"
    );
    assert_eq!(
        serde_json::to_value(&InteractionCallbackError::TemporarilyUnavailable).unwrap(),
        "temporarily_unavailable"
    );
}

#[test]
fn interaction_endpoint_error_serializes_snake_case() {
    assert_eq!(
        serde_json::to_value(&InteractionEndpointError::InteractionUnavailable).unwrap(),
        "interaction_unavailable"
    );
}

#[test]
fn mission_status_error_matches_example() {
    let raw = serde_json::json!({
        "error": "mission_terminated",
        "mission_status": "terminated"
    });
    let err: MissionStatusError = serde_json::from_value(raw).unwrap();
    assert_eq!(err.error, "mission_terminated");
    assert_eq!(err.mission_status, MissionStatus::Terminated);
}
