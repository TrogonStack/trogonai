//! Map `{prefix}.gateway…` ingress NATS subjects to `{prefix}.agent.{agent_id}.{method}` shapes.
//!
//! Tenant isolation uses **one NATS Account per tenant** (see `A2A_PENDING_DECISION.md`); there is no `{tenant}`
//! token on gateway subjects inside an Account — only **`{prefix}.gateway.{agent_id}.{method…}`**.
//!
//! To target a gateway from code that builds agent-shaped subjects (`{prefix}.agent…`), use
//! [`gateway_ingress_subject_from_agent_subject`] (swap **`agent` → `gateway`** on the segment after the prefix).

use std::fmt;

use crate::a2a_prefix::A2aPrefix;
use crate::agent::wire::JsonRpcErrorResponse;
use crate::agent_id::A2aAgentId;

/// Recognized dotted method suffix tokens after `{prefix}.agent.{agent_id}.` /
/// ingress remainder (same spelling as [`crate::agent::dispatch::A2aMethod`] mapping).
///
/// Listed longest-first so `tasks.push_notification_config.*` wins over `tasks.*`.
pub const GATEWAY_INGRESS_METHOD_SUFFIXES: &[&[&str]] = &[
    &["tasks", "push_notification_config", "set"],
    &["tasks", "push_notification_config", "get"],
    &["tasks", "push_notification_config", "list"],
    &["tasks", "push_notification_config", "delete"],
    &["message", "stream"],
    &["message", "send"],
    &["tasks", "resubscribe"],
    &["tasks", "cancel"],
    &["tasks", "list"],
    &["tasks", "get"],
    &["agent", "card"],
];

/// Failure resolving a `{prefix}.gateway.` subject to an agent RPC subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayIngressError {
    /// Subject does not start with `{prefix}.gateway.`.
    NotGatewayIngress,
    /// No agent id / trailing tokens missing after stripping the gateway prefix.
    BadSubjectShape,
    /// Trailing tokens do not match a known A2A method suffix.
    UnknownMethodSuffix,
    /// Agent id segment is not NATS-safe (see [`A2aAgentId`]).
    InvalidAgentId,
}

impl std::fmt::Display for GatewayIngressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotGatewayIngress => write!(
                f,
                "subject does not start with '{{prefix}}.gateway.' for the configured prefix"
            ),
            Self::BadSubjectShape => write!(f, "expected '{{prefix}}.gateway.{{agent_id}}.{{method…}}'"),
            Self::UnknownMethodSuffix => write!(f, "unknown method suffix after gateway segment"),
            Self::InvalidAgentId => write!(f, "agent id segment fails NATS token validation"),
        }
    }
}

impl std::error::Error for GatewayIngressError {}

/// Invalid arguments when assembling `{prefix}.gateway.{agent}.{method…}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayComposeError {
    EmptyMethodTail,
}

impl fmt::Display for GatewayComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMethodTail => write!(f, "gateway ingress method suffix is empty"),
        }
    }
}

impl std::error::Error for GatewayComposeError {}

/// Builds `{prefix}.gateway.{agent_id}.{method…}`.
///
/// `method_suffix_dots` uses the same dotted tail as agent subjects (`"message.send"`,
/// `"tasks.push_notification_config.set"`, …).
pub fn compose_gateway_ingress_subject(
    prefix: &A2aPrefix,
    agent_id: &A2aAgentId,
    method_suffix_dots: &str,
) -> Result<String, GatewayComposeError> {
    let trimmed = method_suffix_dots.trim_matches('.');
    if trimmed.is_empty() {
        return Err(GatewayComposeError::EmptyMethodTail);
    }

    Ok(format!("{}.gateway.{}.{}", prefix.as_str(), agent_id.as_str(), trimmed))
}

/// Maps `{prefix}.agent.{remainder}` → `{prefix}.gateway.{remainder}`.
///
/// Returns `None` when `agent_subject` is not prefixed with `{prefix}.agent.`.
///
/// Passing the result through [`resolve_gateway_ingress_subject`] yields the original `agent_subject`
/// exactly when the trailing tokens match [`GATEWAY_INGRESS_METHOD_SUFFIXES`].
pub fn gateway_ingress_subject_from_agent_subject(agent_subject: &str, prefix: &A2aPrefix) -> Option<String> {
    let leader = format!("{}.agent.", prefix.as_str());
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
    let agent_subject = resolve_gateway_ingress_subject(subject, prefix)?;
    let leader = format!("{}.agent.", prefix.as_str());
    let remainder = agent_subject
        .strip_prefix(&leader)
        .ok_or(GatewayIngressError::BadSubjectShape)?;
    let tokens: Vec<&str> = remainder.split('.').filter(|t| !t.is_empty()).collect();
    if tokens.len() < 2 {
        return Err(GatewayIngressError::BadSubjectShape);
    }
    let agent_id = A2aAgentId::new(tokens[0]).map_err(|_| GatewayIngressError::InvalidAgentId)?;
    Ok((agent_id, tokens[1..].join(".")))
}

/// Resolve ingress subject → core agent RPC subject `{prefix}.agent.{agent_id}.{method…}`.
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
    Ok(format!("{}.agent.{}.{}", prefix.as_str(), agent_id_str, suffix))
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
        // Single segment `{agent}.{method_tokens…}` only (no legacy `{tenant}.{agent}.{method…}` segment).
        if head_len != 1 {
            return None;
        }
        let agent_id = tokens[0];
        if agent_id.is_empty() {
            return None;
        }
        return Some((agent_id, &tokens[head_len..]));
    }
    None
}

fn validate_agent_id(segment: &str) -> Result<(), GatewayIngressError> {
    match A2aAgentId::new(segment) {
        Ok(_) => Ok(()),
        Err(_) => Err(GatewayIngressError::InvalidAgentId),
    }
}

/// Serialize a JSON-RPC error for the caller inbox when ingress routing fails (-32600 Invalid Request).
pub fn ingress_invalid_request_response_bytes(
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> Result<bytes::Bytes, serde_json::Error> {
    let id = crate::extract_request_id(request_payload_hint);
    JsonRpcErrorResponse::new(id, -32600, message.into()).to_bytes()
}

/// Serialize a gateway policy denial reply for the correlating inbox.
pub fn ingress_gateway_policy_denied_response_bytes(
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> Result<bytes::Bytes, serde_json::Error> {
    let id = crate::extract_request_id(request_payload_hint);
    JsonRpcErrorResponse::new(id, -32_801, message.into()).to_bytes()
}

/// Serialize an upstream-gateway deadline overrun (-32800 — reserved for `{prefix}.gateway>` deadlines).
pub fn ingress_gateway_deadline_exceeded_response_bytes(
    request_payload_hint: &[u8],
    message: impl Into<String>,
) -> Result<bytes::Bytes, serde_json::Error> {
    let id = crate::extract_request_id(request_payload_hint);
    JsonRpcErrorResponse::new(id, -32_800, message.into()).to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a_prefix::A2aPrefix;

    fn pfx() -> A2aPrefix {
        A2aPrefix::new("a2a").unwrap()
    }

    #[test]
    fn message_send() {
        assert_eq!(
            resolve_gateway_ingress_subject("a2a.gateway.bot.message.send", &pfx()).unwrap(),
            "a2a.agent.bot.message.send"
        );
    }

    #[test]
    fn legacy_two_segment_identity_area_rejected() {
        assert!(matches!(
            resolve_gateway_ingress_subject("a2a.gateway.acme.bot.message.send", &pfx()),
            Err(GatewayIngressError::UnknownMethodSuffix)
        ));
    }

    #[test]
    fn push_notification_set_four_token_suffix() {
        assert_eq!(
            resolve_gateway_ingress_subject("a2a.gateway.planner.tasks.push_notification_config.set", &pfx()).unwrap(),
            "a2a.agent.planner.tasks.push_notification_config.set"
        );
    }

    #[test]
    fn dotted_prefix() {
        let p = A2aPrefix::new("my.app").unwrap();
        assert_eq!(
            resolve_gateway_ingress_subject("my.app.gateway.planner.tasks.get", &p).unwrap(),
            "my.app.agent.planner.tasks.get"
        );
    }

    #[test]
    fn wrong_prefix_returns_not_gateway() {
        assert!(matches!(
            resolve_gateway_ingress_subject("other.gateway.bot.message.send", &pfx()),
            Err(GatewayIngressError::NotGatewayIngress)
        ));
    }

    #[test]
    fn invalid_agent_id_rejected_instead_of_silent_typo_subject() {
        assert!(matches!(
            resolve_gateway_ingress_subject("a2a.gateway.bad*agent.message.send", &pfx()),
            Err(GatewayIngressError::InvalidAgentId)
        ));
    }

    #[test]
    fn too_many_segments_before_suffix_rejected() {
        assert!(matches!(
            resolve_gateway_ingress_subject("a2a.gateway.t1.t2.bot.message.send", &pfx()),
            Err(GatewayIngressError::UnknownMethodSuffix)
        ));
    }

    #[test]
    fn compose_then_resolve_round_trips() {
        let p = pfx();
        let aid = A2aAgentId::new("planner").unwrap();
        let g = compose_gateway_ingress_subject(&p, &aid, "message.send").unwrap();
        assert_eq!(g, "a2a.gateway.planner.message.send");
        assert_eq!(
            resolve_gateway_ingress_subject(&g, &p).unwrap(),
            "a2a.agent.planner.message.send"
        );
    }

    #[test]
    fn ingress_from_agent_subject_transform() {
        let p = pfx();
        assert_eq!(
            gateway_ingress_subject_from_agent_subject("a2a.agent.planner.message.stream", &p).unwrap(),
            "a2a.gateway.planner.message.stream"
        );
    }

    #[test]
    fn ingress_from_agent_wrong_leader_returns_none() {
        assert!(gateway_ingress_subject_from_agent_subject("a2a.gateway.x.message.send", &pfx()).is_none());
        assert!(gateway_ingress_subject_from_agent_subject("wrong.agent.x.message.send", &pfx()).is_none());
    }

    #[test]
    fn ingress_agent_method_matches_resolve_subject() {
        let p = pfx();
        let subject = "a2a.gateway.planner.message.send";
        let (agent, method_dots) = gateway_ingress_agent_and_method_dots(subject, &p).unwrap();
        assert_eq!(agent.as_str(), "planner");
        assert_eq!(method_dots, "message.send");
        assert_eq!(
            resolve_gateway_ingress_subject(subject, &p).unwrap(),
            "a2a.agent.planner.message.send"
        );
    }

    #[test]
    fn compose_rejects_blank_method_suffix() {
        let p = pfx();
        let aid = A2aAgentId::new("b").unwrap();
        assert!(matches!(
            compose_gateway_ingress_subject(&p, &aid, ""),
            Err(GatewayComposeError::EmptyMethodTail)
        ));
        assert!(matches!(
            compose_gateway_ingress_subject(&p, &aid, "..."),
            Err(GatewayComposeError::EmptyMethodTail)
        ));
    }

    #[test]
    fn invalid_request_payload_produces_stable_jsonrpc_wrapper() {
        let hint = br#"{"jsonrpc":"2.0","id":"x","method":"m"}"#;
        let bytes = ingress_invalid_request_response_bytes(hint, "bad ingress").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], "x");
        assert_eq!(value["error"]["code"], -32600);
        assert!(value["error"]["message"].as_str().unwrap().contains("bad ingress"));
    }
}
