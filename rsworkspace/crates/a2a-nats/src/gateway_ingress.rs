//! Map `{prefix}.gateway…` ingress NATS subjects to `{prefix}.agents.{agent_id}.{method}` shapes.
//!
//! Tenant isolation uses **one NATS Account per tenant** (see [`docs/a2a/explanation/architecture.md`](../../../../docs/a2a/explanation/architecture.md) §Decisions); there is no `{tenant}`
//! token on gateway subjects inside an Account — only **`{prefix}.gateway.{agent_id}.{method…}`**.
//!
//! To target a gateway from code that builds agent-shaped subjects (`{prefix}.agents…`), use
//! [`gateway_ingress_subject_from_agent_subject`] (swap **`agents` → `gateway`** on the segment after the prefix).

use async_nats::header::HeaderMap;
use jsonrpc_nats::Encoded;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::jsonrpc::{JsonRpcId, extract_request_id, extract_request_id_from_body};
use crate::wire::{WireError, encode_error, response_id_from_request_headers};

/// Recognized dotted method suffix tokens after `{prefix}.agents.{agent_id}.` /
/// ingress remainder (same spelling as [`crate::server::dispatch::A2aMethod`] mapping).
///
/// Listed longest-first to ensure deterministic matching.
pub const GATEWAY_INGRESS_METHOD_SUFFIXES: &[&[&str]] = &[
    &["message", "stream"],
    &["message", "send"],
    &["tasks", "resubscribe"],
    &["tasks", "cancel"],
    &["tasks", "list"],
    &["tasks", "get"],
    &["push", "set"],
    &["push", "get"],
    &["push", "list"],
    &["push", "delete"],
    &["card"],
];

/// Failure resolving a `{prefix}.gateway.` subject to an agent RPC subject.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatewayIngressError {
    /// Subject does not start with `{prefix}.gateway.`.
    #[error("subject does not start with '{{prefix}}.gateway.' for the configured prefix")]
    NotGatewayIngress,
    /// No agent id / trailing tokens missing after stripping the gateway prefix.
    #[error("expected '{{prefix}}.gateway.{{agent_id}}.{{method…}}'")]
    BadSubjectShape,
    /// Trailing tokens do not match a known A2A method suffix.
    #[error("unknown method suffix after gateway segment")]
    UnknownMethodSuffix,
    /// Agent id segment is not NATS-safe (see [`A2aAgentId`]).
    #[error("agent id segment fails NATS token validation")]
    InvalidAgentId,
}

/// Invalid arguments when assembling `{prefix}.gateway.{agent}.{method…}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatewayComposeError {
    #[error("gateway ingress method suffix is empty")]
    EmptyMethodTail,
    #[error("gateway ingress method suffix is not a recognised A2A operation")]
    UnknownMethodSuffix,
}

/// Builds `{prefix}.gateway.{agent_id}.{method…}`.
///
/// `method_suffix_dots` uses the same dotted tail as agent subjects (`"message.send"`,
/// `"push.set"`, …) and is rejected with [`GatewayComposeError::UnknownMethodSuffix`]
/// when it doesn't match an entry in [`GATEWAY_INGRESS_METHOD_SUFFIXES`] — that way the
/// composer can't emit a typo-subject that [`resolve_gateway_ingress_subject`] would
/// later refuse on the wire.
pub fn compose_gateway_ingress_subject(
    prefix: &A2aPrefix,
    agent_id: &A2aAgentId,
    method_suffix_dots: &str,
) -> Result<String, GatewayComposeError> {
    let trimmed = method_suffix_dots.trim_matches('.');
    if trimmed.is_empty() {
        return Err(GatewayComposeError::EmptyMethodTail);
    }
    let tokens: Vec<&str> = trimmed.split('.').filter(|t| !t.is_empty()).collect();
    if !GATEWAY_INGRESS_METHOD_SUFFIXES.iter().any(|sfx| tokens == *sfx) {
        return Err(GatewayComposeError::UnknownMethodSuffix);
    }

    Ok(format!("{}.gateway.{}.{}", prefix.as_str(), agent_id.as_str(), trimmed))
}

/// Maps `{prefix}.agents.{remainder}` → `{prefix}.gateway.{remainder}`.
///
/// Returns `None` when `agent_subject` is not prefixed with `{prefix}.agents.`.
///
/// Passing the result through [`resolve_gateway_ingress_subject`] yields the original `agent_subject`
/// exactly when the trailing tokens match [`GATEWAY_INGRESS_METHOD_SUFFIXES`].
pub fn gateway_ingress_subject_from_agent_subject(agent_subject: &str, prefix: &A2aPrefix) -> Option<String> {
    let leader = format!("{}.agents.", prefix.as_str());
    let remainder = agent_subject.strip_prefix(&leader)?;
    (!remainder.is_empty()).then(|| format!("{}.gateway.{remainder}", prefix.as_str()))
}

/// Parses ingress subject → validated [`A2aAgentId`] plus dotted RPC method tail (`message.send`, …).
///
/// Uses the same matching rules as [`resolve_gateway_ingress_subject`].
pub fn gateway_ingress_agent_and_method_dots(
    subject: &str,
    prefix: &A2aPrefix,
) -> Result<(A2aAgentId, String), GatewayIngressError> {
    let leader = format!("{}.gateway.", prefix.as_str());
    let rest = subject
        .strip_prefix(&leader)
        .ok_or(GatewayIngressError::NotGatewayIngress)?;
    if rest.is_empty() {
        return Err(GatewayIngressError::BadSubjectShape);
    }
    let tokens: Vec<&str> = rest.split('.').filter(|t| !t.is_empty()).collect();
    let (agent_id_str, suffix_tokens) =
        peel_agent_and_suffix(&tokens).ok_or(GatewayIngressError::UnknownMethodSuffix)?;
    let agent_id = A2aAgentId::new(agent_id_str).map_err(|_| GatewayIngressError::InvalidAgentId)?;
    Ok((agent_id, suffix_tokens.join(".")))
}

/// Resolve ingress subject → core agent RPC subject `{prefix}.agents.{agent_id}.{method…}`.
pub fn resolve_gateway_ingress_subject(subject: &str, prefix: &A2aPrefix) -> Result<String, GatewayIngressError> {
    let leader = format!("{}.gateway.", prefix.as_str());
    let rest = subject
        .strip_prefix(&leader)
        .ok_or(GatewayIngressError::NotGatewayIngress)?;
    if rest.is_empty() {
        return Err(GatewayIngressError::BadSubjectShape);
    }

    let tokens: Vec<&str> = rest.split('.').filter(|t| !t.is_empty()).collect();
    let (agent_id_str, suffix_tokens) =
        peel_agent_and_suffix(&tokens).ok_or(GatewayIngressError::UnknownMethodSuffix)?;

    validate_agent_id(agent_id_str)?;
    let suffix = suffix_tokens.join(".");
    Ok(format!("{}.agents.{}.{}", prefix.as_str(), agent_id_str, suffix))
}

fn ends_with_suffix(tokens: &[&str], suffix: &[&str]) -> bool {
    tokens.len() >= suffix.len() && tokens[tokens.len() - suffix.len()..] == *suffix
}

/// Returns `(agent_id, suffix_token_slice)` when a known method suffix matches.
fn peel_agent_and_suffix<'a>(tokens: &'a [&'a str]) -> Option<(&'a str, &'a [&'a str])> {
    for sfx in GATEWAY_INGRESS_METHOD_SUFFIXES {
        if !ends_with_suffix(tokens, sfx) {
            continue;
        }
        let head_len = tokens.len() - sfx.len();
        if head_len != 1 {
            return None;
        }
        return Some((tokens[0], &tokens[head_len..]));
    }
    None
}

fn validate_agent_id(segment: &str) -> Result<(), GatewayIngressError> {
    match A2aAgentId::new(segment) {
        Ok(_) => Ok(()),
        Err(_) => Err(GatewayIngressError::InvalidAgentId),
    }
}

fn response_id_for_ingress(request_headers: &HeaderMap, request_payload_hint: &[u8]) -> jsonrpc_nats::ResponseId {
    if let Some(id) = extract_request_id(request_headers) {
        return match id {
            JsonRpcId::Number(n) => jsonrpc_nats::ResponseId::Number(n),
            JsonRpcId::String(s) => jsonrpc_nats::ResponseId::String(s),
            JsonRpcId::Null => jsonrpc_nats::ResponseId::Null,
        };
    }
    match extract_request_id_from_body(request_payload_hint) {
        Some(JsonRpcId::Number(n)) => jsonrpc_nats::ResponseId::Number(n),
        Some(JsonRpcId::String(s)) => jsonrpc_nats::ResponseId::String(s),
        Some(JsonRpcId::Null) | None => jsonrpc_nats::ResponseId::Null,
    }
}

fn ingress_error_wire(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    code: i32,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Result<Encoded, WireError> {
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
) -> Result<bytes::Bytes, WireError> {
    Ok(ingress_error_wire(request_headers, request_payload_hint, -32600, message, None)?.body)
}

/// Serialize a gateway policy denial reply for the correlating inbox.
pub fn ingress_gateway_policy_denied_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> Result<bytes::Bytes, WireError> {
    Ok(ingress_error_wire(request_headers, request_payload_hint, -32_801, message, None)?.body)
}

/// Serialize a Tier-1 declarative policy denial (`-32803`) for the correlating inbox.
pub fn ingress_gateway_declarative_denied_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> Result<bytes::Bytes, WireError> {
    Ok(ingress_error_wire(request_headers, request_payload_hint, -32_803, message, None)?.body)
}

/// Serialize a Tier-3 skill refusal reply (`-32802`) for the correlating inbox.
pub fn ingress_gateway_tier3_refused_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
    rule: &str,
) -> Result<bytes::Bytes, WireError> {
    Ok(ingress_error_wire(
        request_headers,
        request_payload_hint,
        -32_802,
        message,
        Some(serde_json::json!({ "rule": rule })),
    )?
    .body)
}

/// Serialize an upstream-gateway deadline overrun (-32800 — reserved for `{prefix}.gateway>` deadlines).
pub fn ingress_gateway_deadline_exceeded_response_bytes(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> Result<bytes::Bytes, WireError> {
    Ok(ingress_error_wire(request_headers, request_payload_hint, -32_800, message, None)?.body)
}

/// Content-mode wire encoding for ingress error replies (headers + bare error body).
pub fn ingress_error_response_wire(
    request_headers: &HeaderMap,
    request_payload_hint: &[u8],
    code: i32,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) -> Result<Encoded, WireError> {
    ingress_error_wire(request_headers, request_payload_hint, code, message, data)
}

#[cfg(test)]
mod tests;
