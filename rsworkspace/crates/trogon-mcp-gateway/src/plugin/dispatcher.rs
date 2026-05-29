//! NATS request/reply dispatcher for Tier 2.5 policy plugins.

use std::time::{Duration, Instant};

use async_nats::jetstream;
use async_nats::HeaderMap;
use bytes::Bytes;
use serde_json::Value as JsonValue;
use tracing::{instrument, Span};
use trogon_nats::{inject_trace_context, RequestClient};

use crate::audit::{self, AuditEnvelope, IdentityFields};
use crate::authz::IdentitySource;

use super::{
    plugin_subject, PluginCalloutError, PluginDecision, PluginRequest, PluginStage,
};

pub const DEFAULT_CALLOUT_TIMEOUT_MS: u64 = 250;

const AUDIT_OUTCOME_INVOKED: &str = "plugin_invoked";
const AUDIT_OUTCOME_DENIED: &str = "plugin_denied";
const AUDIT_OUTCOME_REWROTE: &str = "plugin_rewrote";
const AUDIT_OUTCOME_FAILED: &str = "plugin_failed";

const AUDIT_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Runtime configuration for plugin callouts.
#[derive(Clone, Debug)]
pub struct PluginDispatcherConfig {
    pub prefix: String,
    pub timeout: Duration,
}

impl Default for PluginDispatcherConfig {
    fn default() -> Self {
        Self {
            prefix: "mcp".into(),
            timeout: Duration::from_millis(DEFAULT_CALLOUT_TIMEOUT_MS),
        }
    }
}

/// Publishes plugin wire envelopes and decodes typed replies.
pub struct PluginDispatcher<'a, N> {
    nats: N,
    config: PluginDispatcherConfig,
    jetstream: Option<&'a jetstream::Context>,
}

impl<'a, N: RequestClient> PluginDispatcher<'a, N> {
    #[must_use]
    pub fn new(nats: N, config: PluginDispatcherConfig) -> Self {
        Self {
            nats,
            config,
            jetstream: None,
        }
    }

    #[must_use]
    pub fn with_audit(mut self, jetstream: &'a jetstream::Context) -> Self {
        self.jetstream = Some(jetstream);
        self
    }

    /// Publish to `{prefix}.plugin.{plugin_name}` and await a typed decision.
    ///
    /// Emits audit events via [`crate::audit`] on every callout. On error, emits
    /// `plugin_failed` with `subject_out` set to `transient` or `permanent` before returning
    /// (deny-bias classification).
    #[instrument(
        name = "mcp.gateway.plugin.call",
        skip(self, req),
        fields(
            plugin.name = %plugin_name,
            plugin.stage = %stage.as_str(),
            mcp.method = %req.mcp_method,
        )
    )]
    pub async fn invoke(
        &self,
        stage: PluginStage,
        plugin_name: &str,
        req: &PluginRequest,
    ) -> Result<PluginDecision, PluginCalloutError> {
        let subject = plugin_subject(&self.config.prefix, plugin_name);
        let started = Instant::now();

        self.emit_audit(
            AUDIT_OUTCOME_INVOKED,
            &subject,
            plugin_name,
            req,
            None,
            None,
        )
        .await;

        let mut wire = req.clone();
        wire.stage = stage;
        if wire.traceparent.is_empty() {
            wire.traceparent = current_traceparent().unwrap_or_default();
        }

        let payload = serde_json::to_vec(&wire).map_err(|e| {
            PluginCalloutError::MalformedReply(format!("request serialize: {e}"))
        })?;

        let mut headers = HeaderMap::new();
        inject_trace_context(&mut headers);
        if !wire.traceparent.is_empty() {
            headers.insert("traceparent", wire.traceparent.as_str());
        }

        let reply = match tokio::time::timeout(
            self.config.timeout,
            self.nats
                .request_with_headers(subject.clone(), headers, Bytes::from(payload)),
        )
        .await
        {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => {
                let err = PluginCalloutError::Transport(e.to_string());
                self.emit_failure_audit(&subject, plugin_name, req, &err).await;
                return Err(err);
            }
            Err(_) => {
                let err = PluginCalloutError::Timeout;
                self.emit_failure_audit(&subject, plugin_name, req, &err).await;
                return Err(err);
            }
        };

        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        let decision = parse_plugin_reply(&reply.payload)?;

        match &decision {
            PluginDecision::Allow => {}
            PluginDecision::Deny { reason } => {
                self.emit_audit(
                    AUDIT_OUTCOME_DENIED,
                    &subject,
                    plugin_name,
                    req,
                    Some(reason.as_str()),
                    Some(latency_ms),
                )
                .await;
            }
            PluginDecision::Rewrite { .. } => {
                self.emit_audit(
                    AUDIT_OUTCOME_REWROTE,
                    &subject,
                    plugin_name,
                    req,
                    None,
                    Some(latency_ms),
                )
                .await;
            }
        }

        Span::current().record("plugin.latency_ms", latency_ms);
        Ok(decision)
    }

    async fn emit_failure_audit(
        &self,
        subject: &str,
        plugin_name: &str,
        req: &PluginRequest,
        err: &PluginCalloutError,
    ) {
        self.emit_audit(
            AUDIT_OUTCOME_FAILED,
            subject,
            plugin_name,
            req,
            Some(err.audit_failure_reason()),
            None,
        )
        .await;
    }

    async fn emit_audit(
        &self,
        outcome: &'static str,
        plugin_subject: &str,
        plugin_name: &str,
        req: &PluginRequest,
        detail: Option<&str>,
        latency_ms: Option<u64>,
    ) {
        let Some(jetstream) = self.jetstream else {
            return;
        };

        let identity_source = req
            .identity
            .source
            .as_deref()
            .map(parse_identity_source)
            .unwrap_or(IdentitySource::Anonymous);

        let identity_fields = IdentityFields {
            agent_id: None,
            agent_version: None,
            wkl: None,
            purpose: None,
            session_id: None,
            act_chain: None,
        };

        let mut envelope = AuditEnvelope::new(
            plugin_subject.to_string(),
            detail.unwrap_or(plugin_name).to_string(),
            outcome,
            "request",
            req.mcp_method.clone(),
            req.identity.tenant.clone(),
            req.identity.caller_sub.clone(),
            None,
            identity_source,
            Some(req.request_id.clone()),
            Some(&identity_fields),
        );

        if let Some(ms) = latency_ms {
            envelope.apply_rate_limit_fields("plugin_callout", ms);
        }

        let method_root = audit::jsonrpc_method_root(&req.mcp_method);
        let audit_subject = audit::audit_publish_subject(&self.config.prefix, outcome, "request", &method_root);
        audit::publish_audit(jetstream, audit_subject, &envelope, AUDIT_ACK_TIMEOUT).await;
    }
}

fn parse_identity_source(raw: &str) -> IdentitySource {
    match raw {
        "jwt" => IdentitySource::Jwt,
        "legacy_header" => IdentitySource::LegacyHeader,
        _ => IdentitySource::Anonymous,
    }
}

fn current_traceparent() -> Option<String> {
    let mut headers = HeaderMap::new();
    inject_trace_context(&mut headers);
    headers
        .get("traceparent")
        .map(|v| v.as_str().to_string())
}

fn parse_plugin_reply(payload: &[u8]) -> Result<PluginDecision, PluginCalloutError> {
    let value: JsonValue = serde_json::from_slice(payload)
        .map_err(|e| PluginCalloutError::MalformedReply(e.to_string()))?;

    let decision = value
        .get("decision")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            PluginCalloutError::ProtocolMismatch("missing decision field".into())
        })?;

    match decision {
        "allow" => Ok(PluginDecision::Allow),
        "deny" => {
            let reason = value
                .get("reason")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| {
                    PluginCalloutError::ProtocolMismatch("deny requires reason".into())
                })?;
            Ok(PluginDecision::Deny {
                reason: reason.to_string(),
            })
        }
        "rewrite" => {
            let params = value.get("params").cloned().ok_or_else(|| {
                PluginCalloutError::ProtocolMismatch("rewrite requires params".into())
            })?;
            let result = value.get("result").cloned();
            Ok(PluginDecision::Rewrite { params, result })
        }
        other => Err(PluginCalloutError::ProtocolMismatch(format!(
            "unknown decision: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_nats::HeaderMap;
    use bytes::Bytes;
    use trogon_nats::RequestClient;

    use super::*;
    use crate::plugin::{PluginIdentity, PluginRequest, PluginStage};

    #[derive(Clone, Default)]
    struct MockRequestClient {
        responses: Arc<Mutex<HashMap<String, Bytes>>>,
        hang: Arc<Mutex<bool>>,
        fail: Arc<Mutex<bool>>,
    }

    impl MockRequestClient {
        fn set_response(&self, subject: &str, body: impl Into<Bytes>) {
            self.responses
                .lock()
                .expect("lock")
                .insert(subject.to_string(), body.into());
        }

        fn hang_next(&self) {
            *self.hang.lock().expect("lock") = true;
        }

        fn fail_next(&self) {
            *self.fail.lock().expect("lock") = true;
        }
    }

    impl RequestClient for MockRequestClient {
        type RequestError = std::io::Error;

        async fn request_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            _headers: HeaderMap,
            _payload: Bytes,
        ) -> Result<async_nats::Message, Self::RequestError> {
            let subject = subject.to_subject().to_string();
            if *self.hang.lock().expect("lock") {
                *self.hang.lock().expect("lock") = false;
                std::future::pending::<()>().await;
            }
            if *self.fail.lock().expect("lock") {
                *self.fail.lock().expect("lock") = false;
                return Err(std::io::Error::other("transport"));
            }
            let payload = self
                .responses
                .lock()
                .expect("lock")
                .get(&subject)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("no response for {subject}")))?;
            Ok(async_nats::Message {
                subject: subject.into(),
                reply: None,
                payload,
                headers: None,
                length: 0,
                status: None,
                description: None,
            })
        }
    }

    fn sample_request() -> PluginRequest {
        PluginRequest {
            request_id: serde_json::json!("req-1"),
            stage: PluginStage::PreCall,
            mcp_method: "tools/call".into(),
            params: serde_json::json!({"name": "grep"}),
            identity: PluginIdentity {
                tenant: Some("acme".into()),
                caller_sub: Some("user:alice".into()),
                source: Some("jwt".into()),
            },
            claims: serde_json::json!({}),
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
        }
    }

    #[tokio::test]
    async fn dispatcher_happy_path_allow() {
        let nats = MockRequestClient::default();
        nats.set_response(
            "mcp.plugin.risk-scorer",
            br#"{"decision":"allow"}"#.as_slice(),
        );

        let dispatcher = PluginDispatcher::new(
            nats,
            PluginDispatcherConfig {
                prefix: "mcp".into(),
                timeout: Duration::from_millis(250),
            },
        );

        let decision = dispatcher
            .invoke(PluginStage::PreAuthz, "risk-scorer", &sample_request())
            .await
            .expect("invoke");

        assert_eq!(decision, PluginDecision::Allow);
    }

    #[tokio::test]
    async fn timeout_is_transient_deny_bias() {
        let nats = MockRequestClient::default();
        nats.hang_next();

        let dispatcher = PluginDispatcher::new(
            nats,
            PluginDispatcherConfig {
                prefix: "mcp".into(),
                timeout: Duration::from_millis(10),
            },
        );

        let err = dispatcher
            .invoke(PluginStage::PreCall, "redaction", &sample_request())
            .await
            .expect_err("timeout");

        assert_eq!(err, PluginCalloutError::Timeout);
        assert!(err.is_transient());
        assert_eq!(err.audit_failure_reason(), "transient");
    }

    #[tokio::test]
    async fn transport_error_is_transient() {
        let nats = MockRequestClient::default();
        nats.fail_next();

        let dispatcher = PluginDispatcher::new(
            nats,
            PluginDispatcherConfig {
                prefix: "mcp".into(),
                timeout: Duration::from_millis(50),
            },
        );

        let err = dispatcher
            .invoke(PluginStage::PreCall, "redaction", &sample_request())
            .await
            .expect_err("transport");

        assert!(matches!(err, PluginCalloutError::Transport(_)));
        assert!(err.is_transient());
    }

    #[tokio::test]
    async fn malformed_reply_is_permanent() {
        let nats = MockRequestClient::default();
        nats.set_response("mcp.plugin.redaction", Bytes::from_static(b"not-json"));

        let dispatcher = PluginDispatcher::new(
            nats,
            PluginDispatcherConfig {
                prefix: "mcp".into(),
                timeout: Duration::from_millis(50),
            },
        );

        let err = dispatcher
            .invoke(PluginStage::PreCall, "redaction", &sample_request())
            .await
            .expect_err("malformed");

        assert!(matches!(err, PluginCalloutError::MalformedReply(_)));
        assert!(!err.is_transient());
        assert_eq!(err.audit_failure_reason(), "permanent");
    }

    #[tokio::test]
    async fn protocol_mismatch_is_permanent() {
        let nats = MockRequestClient::default();
        nats.set_response(
            "mcp.plugin.redaction",
            br#"{"decision":"deny"}"#.as_slice(),
        );

        let dispatcher = PluginDispatcher::new(
            nats,
            PluginDispatcherConfig {
                prefix: "mcp".into(),
                timeout: Duration::from_millis(50),
            },
        );

        let err = dispatcher
            .invoke(PluginStage::PreCall, "redaction", &sample_request())
            .await
            .expect_err("protocol");

        assert!(matches!(err, PluginCalloutError::ProtocolMismatch(_)));
        assert_eq!(err.audit_failure_reason(), "permanent");
    }

    #[test]
    fn parse_reply_rewrite() {
        let decision = parse_plugin_reply(
            br#"{"decision":"rewrite","params":{"x":1},"result":{"ok":true}}"#,
        )
        .expect("parse");
        assert!(matches!(decision, PluginDecision::Rewrite { .. }));
    }
}
