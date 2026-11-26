use std::time::Duration;

use async_nats::jetstream::{self, stream};
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::CredentialAuditEvent;
use trogon_sandbox_protocol::subjects::{CREDENTIAL_AUDIT_STREAM, credential_audit_stream_name};

use crate::{AuditError, AuditLogger, BackendError};

const RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// Audit events published to a JetStream stream, kept for a year.
pub struct JetStreamAuditLogger {
    jetstream: jetstream::Context,
}

impl JetStreamAuditLogger {
    /// Creates the audit stream when it does not exist yet.
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, AuditError> {
        jetstream
            .get_or_create_stream(stream::Config {
                name: credential_audit_stream_name(),
                subjects: vec![format!("{CREDENTIAL_AUDIT_STREAM}.>")],
                retention: stream::RetentionPolicy::Limits,
                max_age: RETENTION,
                ..Default::default()
            })
            .await
            .map_err(BackendError::new)?;
        Ok(Self { jetstream })
    }
}

#[async_trait]
impl AuditLogger for JetStreamAuditLogger {
    async fn emit(&self, event: CredentialAuditEvent) -> Result<(), AuditError> {
        let payload = Bytes::from(serde_json::to_vec(&event)?);
        let ack = self
            .jetstream
            .publish(format!("{CREDENTIAL_AUDIT_STREAM}.event"), payload)
            .await
            .map_err(BackendError::new)?;
        ack.await.map_err(BackendError::new)?;
        Ok(())
    }
}
