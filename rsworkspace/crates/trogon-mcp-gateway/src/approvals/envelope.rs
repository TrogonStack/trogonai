use serde_json::{Value, json};

use crate::approvals::types::{ApprovalSubject, RequestId};
use crate::rpc_codes;

#[must_use]
pub fn build_approval_required(
    prefix: &str,
    request_id: &RequestId,
    reason: &str,
    ttl_s: u64,
    approval_base_url: &str,
) -> Value {
    build_approval_required_with_subject(
        request_id,
        &ApprovalSubject::for_request(prefix, request_id),
        reason,
        ttl_s,
        approval_base_url,
    )
}

#[must_use]
pub fn build_approval_required_step_up(
    prefix: &str,
    request_id: &RequestId,
    scope_required: &str,
    ttl_s: u64,
    approval_base_url: &str,
) -> Value {
    build_approval_required_with_subject(
        request_id,
        &ApprovalSubject::for_step_up(prefix, request_id),
        scope_required,
        ttl_s,
        approval_base_url,
    )
}

#[must_use]
pub fn build_approval_required_with_subject(
    request_id: &RequestId,
    subject: &ApprovalSubject,
    reason: &str,
    ttl_s: u64,
    approval_base_url: &str,
) -> Value {
    let approval_url = format!(
        "{}/approvals/{}",
        approval_base_url.trim_end_matches('/'),
        request_id.as_str()
    );
    json!({
        "approval_url": approval_url,
        "approval_subject": subject.as_str(),
        "request_id": request_id.as_str(),
        "ttl_seconds": ttl_s,
        "reason": reason,
    })
}

#[must_use]
pub fn jsonrpc_error_with_approval_data(
    id: Option<Value>,
    data: Value,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": rpc_codes::APPROVAL_REQUIRED,
            "message": "approval_required",
            "data": data,
        }
    })
}

#[must_use]
pub fn jsonrpc_error_rate_limited(id: Option<Value>, retry_after_s: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": rpc_codes::RATE_LIMITED,
            "message": "rate_limited",
            "data": {
                "retry_after_s": retry_after_s,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::types::RequestId;

    #[test]
    fn approval_required_envelope_shape() {
        let request_id = RequestId::new("req-abc").unwrap();
        let body = build_approval_required("mcp", &request_id, "high_risk_tool", 300, "https://console.example");
        assert_eq!(body["approval_subject"], "mcp.approvals.req-abc");
        assert_eq!(body["request_id"], "req-abc");
        assert_eq!(body["ttl_seconds"], 300);
        assert_eq!(body["reason"], "high_risk_tool");
        assert!(body["approval_url"]
            .as_str()
            .unwrap()
            .ends_with("/approvals/req-abc"));
    }

    #[test]
    fn approval_subject_respects_custom_prefix() {
        let request_id = RequestId::new("req-xyz").unwrap();
        let body = build_approval_required("acme.mcp", &request_id, "policy", 60, "https://console.example");
        assert_eq!(body["approval_subject"], "acme.mcp.approvals.req-xyz");
    }

    #[test]
    fn step_up_uses_step_up_subject() {
        let request_id = RequestId::new("req-step").unwrap();
        let body = build_approval_required_step_up("mcp", &request_id, "scope:admin", 120, "https://console.example");
        assert_eq!(body["approval_subject"], "mcp.approvals.step-up.req-step");
        assert_eq!(body["reason"], "scope:admin");
    }

    #[test]
    fn jsonrpc_wraps_approval_data() {
        let request_id = RequestId::new("req-wrap").unwrap();
        let data = build_approval_required("mcp", &request_id, "policy", 60, "https://console.example");
        let err = jsonrpc_error_with_approval_data(Some(json!(1)), data);
        assert_eq!(err["error"]["code"], rpc_codes::APPROVAL_REQUIRED);
        assert_eq!(err["error"]["message"], "approval_required");
        assert_eq!(err["error"]["data"]["request_id"], "req-wrap");
    }
}
