use std::time::Duration;

use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::{self, Context};
use async_trait::async_trait;
use futures::StreamExt;
use tracing::{debug, warn};

use crate::envelope::AuditEnvelope;
use crate::error::ProjectorError;
use crate::projector::{AuditConsumer, AuditProjector, DURABLE_CONSUMER};

pub struct JetStreamAuditConsumer<P: AuditProjector> {
    jetstream: Context,
    stream_name: String,
    projector: P,
}

impl<P: AuditProjector> JetStreamAuditConsumer<P> {
    pub fn new(jetstream: Context, stream_name: impl Into<String>, projector: P) -> Self {
        Self {
            jetstream,
            stream_name: stream_name.into(),
            projector,
        }
    }

    async fn consumer(&self) -> Result<jetstream::consumer::Consumer<pull::Config>, ProjectorError> {
        let stream = self
            .jetstream
            .get_stream(&self.stream_name)
            .await
            .map_err(|error| ProjectorError::Consumer(error.to_string()))?;
        stream
            .create_consumer(pull::Config {
                durable_name: Some(DURABLE_CONSUMER.into()),
                filter_subject: "mcp.audit.>".into(),
                ack_policy: jetstream::consumer::AckPolicy::Explicit,
                ..Default::default()
            })
            .await
            .map_err(|error| ProjectorError::Consumer(error.to_string()))
    }
}

#[async_trait]
impl<P: AuditProjector + Send + Sync> AuditConsumer for JetStreamAuditConsumer<P> {
    async fn run(&self) -> Result<(), ProjectorError> {
        let consumer = self.consumer().await?;
        let mut messages = consumer
            .messages()
            .await
            .map_err(|error| ProjectorError::Consumer(error.to_string()))?;

        while let Some(message) = messages.next().await {
            let message = message.map_err(|error| ProjectorError::Consumer(error.to_string()))?;
            let subject = message.subject.to_string();
            if let Err(error) = self.handle_message(&subject, &message.payload).await {
                warn!(%subject, %error, "failed to project audit envelope");
                continue;
            }
            if let Err(error) = message.ack().await {
                warn!(%subject, %error, "failed to ack projected audit envelope");
            } else {
                debug!(%subject, "projected audit envelope");
            }
        }
        Ok(())
    }

    async fn handle_message(&self, subject: &str, payload: &[u8]) -> Result<(), ProjectorError> {
        if !subject.starts_with("mcp.audit.sts.") && !subject.starts_with("mcp.audit.registry.") {
            return Ok(());
        }
        let envelope = AuditEnvelope::from_message(subject, payload)?;
        self.projector.project(envelope).await
    }
}

pub async fn ensure_audit_stream(jetstream: &Context, stream_name: &str, prefix: &str) -> Result<(), ProjectorError> {
    let subject = format!("{prefix}.audit.>");
    jetstream
        .get_or_create_stream(jetstream::stream::Config {
            name: stream_name.to_string(),
            subjects: vec![subject],
            max_messages: 100_000,
            ..Default::default()
        })
        .await
        .map_err(|error| ProjectorError::Consumer(error.to_string()))?;
    Ok(())
}

pub const DEFAULT_ACK_WAIT: Duration = Duration::from_secs(30);
