//! Gateway ingress helpers: the agent-to-gateway subject overlay and the JSON-RPC
//! error replies the gateway sends when it refuses an ingress request.
//!
//! Tenant isolation uses **one NATS Account per tenant** (see [`docs/a2a/explanation/architecture.md`](../../../../../docs/a2a/explanation/architecture.md) §Decisions); there is no `{tenant}`
//! token on gateway subjects inside an Account, only
//! **`{prefix}.v1.gateway.{agent_id}.{kind}.{id}.{method…}`**, which
//! [`GatewayIngressSubject`](crate::nats::subjects::gateway::GatewayIngressSubject) builds and parses.
//!
//! To target a gateway from code that builds agent-shaped subjects (`{prefix}.v1.agents…`), use
//! [`gateway_ingress_subject_from_agent_subject`] (swap **`agents` → `gateway`** on the segment after the prefix).

use async_nats::header::HeaderMap;
use jsonrpc_nats::Encoded;

use crate::a2a_prefix::A2aPrefix;
use crate::jsonrpc::{extract_request_id, extract_request_id_from_body};
use crate::wire::{encode_error, response_id_from_request_headers};

/// Maps `{prefix}.v1.agents.{remainder}` → `{prefix}.v1.gateway.{remainder}`.
///
/// Returns `None` when `agent_subject` is not prefixed with `{prefix}.v1.agents.`.
///
/// The remainder keeps the caller's principal position, so the gateway relays the
/// request to the same agent subject it was mapped from:
/// [`GatewayIngressSubject::agent_delivery`](crate::nats::subjects::gateway::GatewayIngressSubject::agent_delivery)
/// of the parsed result renders `agent_subject` again.
pub fn gateway_ingress_subject_from_agent_subject(agent_subject: &str, prefix: &A2aPrefix) -> Option<String> {
    let leader = format!("{}.v1.agents.", prefix.as_str());
    let remainder = agent_subject.strip_prefix(&leader)?;
    (!remainder.is_empty()).then(|| format!("{}.v1.gateway.{remainder}", prefix.as_str()))
}

fn response_id_for_ingress(request_headers: &HeaderMap, request_payload_hint: &[u8]) -> jsonrpc_nats::ResponseId {
    extract_request_id(request_headers)
        .or_else(|| extract_request_id_from_body(request_payload_hint))
        .unwrap_or(jsonrpc_nats::ResponseId::Null)
}

fn ingress_error_wire(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    code: i32,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Encoded {
    let id = if request_headers.get(jsonrpc_nats::HEADER_ID).is_some() {
        response_id_from_request_headers(request_headers)
    } else {
        response_id_for_ingress(request_headers, request_payload_hint)
    };
    encode_error(id, code, message, data)
}

/// Serialize a JSON-RPC error for the caller inbox when ingress routing fails (-32600 Invalid Request).
pub fn ingress_invalid_request_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> bytes::Bytes {
    ingress_error_wire(request_headers, request_payload_hint, -32600, message, None).body
}

/// Serialize a gateway policy denial reply for the correlating inbox.
pub fn ingress_gateway_policy_denied_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> bytes::Bytes {
    ingress_error_wire(request_headers, request_payload_hint, -32_801, message, None).body
}

/// Serialize a Tier-1 declarative policy denial (`-32803`) for the correlating inbox.
pub fn ingress_gateway_declarative_denied_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> bytes::Bytes {
    ingress_error_wire(request_headers, request_payload_hint, -32_803, message, None).body
}

struct Tier3RefusalData<'a> {
    rule: &'a str,
}

impl From<Tier3RefusalData<'_>> for serde_json::Value {
    fn from(data: Tier3RefusalData<'_>) -> Self {
        let mut fields = serde_json::Map::new();
        fields.insert("rule".to_owned(), Self::String(data.rule.to_owned()));
        Self::Object(fields)
    }
}

/// Serialize a Tier-3 skill refusal reply (`-32802`) for the correlating inbox.
pub fn ingress_gateway_tier3_refused_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
    rule: &str,
) -> bytes::Bytes {
    ingress_error_wire(
        request_headers,
        request_payload_hint,
        -32_802,
        message,
        Some(Tier3RefusalData { rule }.into()),
    )
    .body
}

/// Serialize an upstream-gateway deadline overrun (-32800, reserved for
/// `{prefix}.v1.gateway.>` deadlines).
pub fn ingress_gateway_deadline_exceeded_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> bytes::Bytes {
    ingress_error_wire(request_headers, request_payload_hint, -32_800, message, None).body
}

/// Canonical wire encoding for ingress error replies (complete JSON-RPC body).
pub fn ingress_error_response_wire(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    code: i32,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Encoded {
    ingress_error_wire(request_headers, request_payload_hint, code, message, data)
}

#[cfg(test)]
mod tests;
