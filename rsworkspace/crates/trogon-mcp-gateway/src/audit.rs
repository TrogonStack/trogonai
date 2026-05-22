//! JetStream-backed audit publishing for gateway decisions.

use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::stream;
use serde::Serialize;
use tracing::warn;

use crate::authz::IdentitySource;

#[derive(Debug, Serialize)]
pub struct AuditEnvelope {
    pub subject_in: String,
    pub subject_out: String,
    pub outcome: &'static str,
    pub direction: &'static str,
    pub jsonrpc_method: String,
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_sub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt_issuer: Option<String>,
    pub identity_source: IdentitySource,
    pub request_id: Option<serde_json::Value>,
}

pub fn audit_publish_subject(prefix: &str, outcome: &str, direction: &str, method_root: &str) -> String {
    format!("{prefix}.audit.{outcome}.{direction}.{method_root}")
}

pub fn jsonrpc_method_root(method: &str) -> String {
    method
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .replace('.', "_")
}

pub async fn ensure_audit_stream(
    jetstream: &jetstream::Context,
    stream_name: &str,
    prefix: &str,
) -> Result<(), jetstream::context::CreateStreamError> {
    let subject = format!("{prefix}.audit.>");
    jetstream
        .get_or_create_stream(stream::Config {
            name: stream_name.to_string(),
            subjects: vec![subject],
            max_messages: 100_000,
            ..Default::default()
        })
        .await?;
    Ok(())
}

pub async fn publish_audit(
    jetstream: &jetstream::Context,
    subject: String,
    envelope: &AuditEnvelope,
    ack_timeout: Duration,
) {
    let body = match serde_json::to_vec(envelope) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "audit envelope serialization failed");
            return;
        }
    };
    let Ok(pending) = jetstream.publish(subject, body.into()).await else {
        warn!("audit jetstream publish failed");
        return;
    };
    match tokio::time::timeout(ack_timeout, pending).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = %e, "audit publish ack failed"),
        Err(_) => warn!(?ack_timeout, "audit publish ack timed out"),
    }
}
