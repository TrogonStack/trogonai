//! Audit envelope emitted for every dispatched request, including
//! backend failures. The subject mirrors the MCP gateway shape
//! (`{prefix}.frontend.audit.{outcome}.{backend}`) so existing audit
//! consumers can pick it up without code changes.

use serde::{Deserialize, Serialize};

use crate::routes::Backend;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrontendAuditOutcome {
    Dispatched,
    UpstreamError,
    Denied,
    NotFound,
}

impl FrontendAuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::UpstreamError => "upstream_error",
            Self::Denied => "denied",
            Self::NotFound => "not_found",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontendAuditEvent {
    pub request_id: String,
    pub tenant: String,
    pub backend: Backend,
    pub upstream_url: String,
    pub method: String,
    pub path: String,
    pub outcome: FrontendAuditOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}

pub fn audit_subject(prefix: &str, outcome: FrontendAuditOutcome, backend: Backend) -> String {
    format!("{prefix}.frontend.audit.{}.{}", outcome.as_str(), backend.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_subject_shape() {
        assert_eq!(
            audit_subject("mcp", FrontendAuditOutcome::Dispatched, Backend::Mcp),
            "mcp.frontend.audit.dispatched.mcp"
        );
        assert_eq!(
            audit_subject("mcp", FrontendAuditOutcome::UpstreamError, Backend::A2a),
            "mcp.frontend.audit.upstream_error.a2a"
        );
    }

    #[test]
    fn event_serde_round_trip() {
        let event = FrontendAuditEvent {
            request_id: "req-1".into(),
            tenant: "acme".into(),
            backend: Backend::Mcp,
            upstream_url: "https://mcp.internal".into(),
            method: "POST".into(),
            path: "/messages".into(),
            outcome: FrontendAuditOutcome::Dispatched,
            upstream_status: Some(200),
            error: None,
            traceparent: Some("00-x-y-01".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: FrontendAuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }
}
