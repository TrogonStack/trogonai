//! Tier 2.5 NATS-callout policy plugins on `{prefix}.plugin.{plugin_name}`.
//!
//! Escape hatch between CEL (Tier 2) and WASM components (Tier 3): the gateway publishes a
//! request envelope at well-defined pipeline hooks and awaits a typed reply that may **Allow**,
//! **Deny**, or **Rewrite** the in-flight MCP JSON-RPC request.
//!
//! # Wire envelope (gateway → plugin)
//!
//! Stable JSON request body published via NATS request/reply. Fields are intentionally flat so
//! any language can subscribe on `mcp.plugin.{plugin_name}` (queue group `mcp-plugin-{plugin_name}`).
//!
//! ```json
//! {
//!   "request_id": "<json-rpc id or correlation string>",
//!   "stage": "pre-authz" | "pre-call" | "post-call",
//!   "mcp_method": "tools/call",
//!   "params": { "...": "opaque JSON-RPC params blob" },
//!   "identity": {
//!     "tenant": "acme",
//!     "caller_sub": "user:alice",
//!     "source": "jwt"
//!   },
//!   "claims": { "...": "opaque JWT / mesh claims snapshot" },
//!   "traceparent": "00-<32-hex-trace-id>-<16-hex-span-id>-<flags>"
//! }
//! ```
//!
//! `traceparent` MUST be present so the plugin subscriber can continue the W3C trace started at
//! gateway ingress (ADR 0032). NATS message headers also carry trace context via
//! [`trogon_nats::inject_trace_context`].
//!
//! # Wire reply (plugin → gateway)
//!
//! ```json
//! { "decision": "allow" }
//! { "decision": "deny", "reason": "<opaque operator-facing category>" }
//! {
//!   "decision": "rewrite",
//!   "params": { "...": "replacement params" },
//!   "result": { "...": "optional pre-computed result for post-call hooks" }
//! }
//! ```
//!
//! Default callout timeout: **250 ms** (ADR 0011 policy-callout tier; distinct from auth callout).

pub mod dispatcher;
pub mod errors;
pub mod registry;

pub use dispatcher::{PluginDispatcher, PluginDispatcherConfig, DEFAULT_CALLOUT_TIMEOUT_MS};
pub use errors::{PluginCalloutError, PluginFailureClass};
pub use registry::{PluginChainSpec, PluginRegistry};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Pipeline hook where a plugin may run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginStage {
    PreAuthz,
    PreCall,
    PostCall,
}

impl PluginStage {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreAuthz => "pre-authz",
            Self::PreCall => "pre-call",
            Self::PostCall => "post-call",
        }
    }
}

/// Caller identity snapshot embedded in the wire envelope.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_sub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// In-flight MCP request context passed to a plugin callout.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginRequest {
    pub request_id: JsonValue,
    pub stage: PluginStage,
    pub mcp_method: String,
    pub params: JsonValue,
    pub identity: PluginIdentity,
    #[serde(default)]
    pub claims: JsonValue,
    pub traceparent: String,
}

/// Typed plugin reply decoded from the wire envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "lowercase")]
pub enum PluginDecision {
    Allow,
    Deny {
        reason: String,
    },
    Rewrite {
        params: JsonValue,
        #[serde(default)]
        result: Option<JsonValue>,
    },
}

/// Metadata about a completed plugin callout (latency, subject, stage).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginInvocation {
    pub stage: PluginStage,
    pub plugin_name: String,
    pub subject: String,
    pub latency_ms: u64,
}

/// NATS subject for a named plugin.
#[must_use]
pub fn plugin_subject(prefix: &str, plugin_name: &str) -> String {
    format!("{prefix}.plugin.{plugin_name}")
}

/// Validate `{plugin_name}` per reference-subject-grammar: `[a-z0-9-]{1,64}`.
#[must_use]
pub fn is_valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_subject_format() {
        assert_eq!(plugin_subject("mcp", "risk-scorer"), "mcp.plugin.risk-scorer");
    }

    #[test]
    fn valid_plugin_name_rules() {
        assert!(is_valid_plugin_name("redaction"));
        assert!(is_valid_plugin_name("risk-scorer"));
        assert!(!is_valid_plugin_name(""));
        assert!(!is_valid_plugin_name("Bad_Name"));
        assert!(!is_valid_plugin_name("a".repeat(65).as_str()));
    }

    #[test]
    fn wire_envelope_round_trip() {
        let req = PluginRequest {
            request_id: serde_json::json!("req-1"),
            stage: PluginStage::PreCall,
            mcp_method: "tools/call".into(),
            params: serde_json::json!({"name": "grep"}),
            identity: PluginIdentity {
                tenant: Some("acme".into()),
                caller_sub: Some("user:alice".into()),
                source: Some("jwt".into()),
            },
            claims: serde_json::json!({"sub": "user:alice"}),
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
        };
        let wire = serde_json::to_string(&req).expect("serialize");
        let restored: PluginRequest = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(restored, req);
    }

    #[test]
    fn decision_reply_variants_round_trip() {
        let allow = PluginDecision::Allow;
        let deny = PluginDecision::Deny {
            reason: "policy".into(),
        };
        let rewrite = PluginDecision::Rewrite {
            params: serde_json::json!({"x": 1}),
            result: None,
        };
        for decision in [allow, deny, rewrite] {
            let wire = serde_json::to_string(&decision).expect("serialize");
            let restored: PluginDecision = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(restored, decision);
        }
    }
}
