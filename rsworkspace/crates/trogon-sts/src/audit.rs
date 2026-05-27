use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tracing::warn;

use crate::error::StsError;
use crate::types::{MintedClaimsSummary, StsExchangeRequest};

#[derive(Debug, Clone, Serialize)]
pub struct StsAuditEvent {
    pub outcome: String,
    pub decision_reason: String,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
    pub request: StsAuditRequestFields,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minted: Option<MintedClaimsSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wkl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StsAuditRequestFields {
    pub subject_token_type: String,
    pub actor_token: String,
    pub audience: String,
    pub scope: String,
    pub purpose: String,
    pub requested_token_type: String,
}

impl From<&StsExchangeRequest> for StsAuditRequestFields {
    fn from(req: &StsExchangeRequest) -> Self {
        Self {
            subject_token_type: req.subject_token_type.clone(),
            actor_token: req.actor_token.clone(),
            audience: req.audience.clone(),
            scope: req.scope.clone(),
            purpose: req.purpose.clone(),
            requested_token_type: req.requested_token_type.clone(),
        }
    }
}

pub fn audit_subject(outcome: &str) -> String {
    format!("mcp.audit.sts.{outcome}")
}

#[async_trait]
pub trait AuditPublisher: Send + Sync {
    async fn publish(&self, event: StsAuditEvent) -> Result<(), StsError>;
}

#[derive(Clone)]
pub struct NatsAuditPublisher<C> {
    client: C,
}

impl<C> NatsAuditPublisher<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<C> AuditPublisher for NatsAuditPublisher<C>
where
    C: trogon_nats::client::PublishClient + trogon_nats::client::FlushClient + Send + Sync + Clone,
{
    async fn publish(&self, event: StsAuditEvent) -> Result<(), StsError> {
        let subject = audit_subject(&event.outcome);
        trogon_nats::messaging::publish(
            &self.client,
            &subject,
            &event,
            trogon_nats::messaging::PublishOptions::simple(),
        )
        .await
        .map_err(|e| StsError::ServerError(format!("audit publish: {e}")))?;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct LogAuditPublisher;

#[async_trait]
impl AuditPublisher for LogAuditPublisher {
    async fn publish(&self, event: StsAuditEvent) -> Result<(), StsError> {
        warn!(
            event = "sts_audit",
            outcome = %event.outcome,
            reason = %event.decision_reason,
            latency_ms = event.latency_ms,
            "sts exchange audit"
        );
        Ok(())
    }
}

pub struct RecordingAuditPublisher {
    events: std::sync::Mutex<Vec<StsAuditEvent>>,
}

impl RecordingAuditPublisher {
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn take_events(&self) -> Vec<StsAuditEvent> {
        std::mem::take(&mut self.events.lock().expect("audit lock"))
    }
}

impl Default for RecordingAuditPublisher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditPublisher for RecordingAuditPublisher {
    async fn publish(&self, event: StsAuditEvent) -> Result<(), StsError> {
        self.events.lock().expect("audit lock").push(event);
        Ok(())
    }
}

pub struct StsAuditEmit<'a> {
    pub outcome: &'a str,
    pub reason: String,
    pub latency: Duration,
    pub request: &'a StsExchangeRequest,
    pub wkl: Option<String>,
    pub agent_id: Option<String>,
    pub minted: Option<MintedClaimsSummary>,
    pub source_ip: Option<String>,
}

pub async fn emit_audit<P: AuditPublisher>(publisher: &P, ctx: StsAuditEmit<'_>) {
    let event = StsAuditEvent {
        outcome: ctx.outcome.to_string(),
        decision_reason: ctx.reason,
        latency_ms: ctx.latency.as_millis() as u64,
        source_ip: ctx.source_ip,
        request: ctx.request.into(),
        minted: ctx.minted,
        wkl: ctx.wkl,
        agent_id: ctx.agent_id,
    };
    if let Err(e) = publisher.publish(event).await {
        warn!(error = %e, "failed to publish sts audit event");
    }
}
