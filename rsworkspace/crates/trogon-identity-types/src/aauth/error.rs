//! Shared error response shapes per draft "Error Responses" (#error-response-format),
//! "Token Endpoint Error Codes", "Polling Error Codes", "Interaction Callback Errors",
//! "Interaction Endpoint Errors", and "Mission Status Errors".

use serde::{Deserialize, Serialize};

/// RFC 9457 `application/problem+json` body as profiled by "Error Response Format".
/// `error` is the AAuth-specific extension member; other RFC 9457 members
/// (`type`, `title`, `status`, `instance`) are modeled conservatively as optional
/// strings since the draft gives no concrete example combining them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl ErrorResponse {
    #[must_use]
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            detail: None,
            type_: None,
            title: None,
            status: None,
            instance: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Token endpoint error codes per "Token Endpoint Error Codes".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenEndpointError {
    InvalidRequest,
    InvalidAgentToken,
    ExpiredAgentToken,
    InvalidResourceToken,
    ExpiredResourceToken,
    UserUnreachable,
    ServerError,
}

/// Polling error codes per "Polling Error Codes".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PollingError {
    Denied,
    Abandoned,
    Expired,
    InvalidCode,
    SlowDown,
    ServerError,
}

/// Interaction callback error codes per "Interaction Callback Errors".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionCallbackError {
    AccessDenied,
    UserAbandoned,
    ServerError,
    TemporarilyUnavailable,
    InteractionExpired,
}

/// Interaction endpoint error codes per "Interaction Endpoint Errors".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionEndpointError {
    InteractionUnavailable,
}

/// Mission status values per "Mission Management".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    Active,
    Terminated,
}

/// Mission status error body per "Mission Status Errors".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionStatusError {
    pub error: String,
    pub mission_status: MissionStatus,
}

#[cfg(test)]
mod tests {
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
}
