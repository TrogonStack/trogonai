use async_nats::jetstream::{self, stream};
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::SnapshotTagAuditEvent;
use trogon_sandbox_protocol::subjects::{SNAPSHOT_TAG_AUDIT_STREAM, snapshot_tag_audit_subject};

use super::snapshot_tag_audit_error::SnapshotTagAuditError;
use super::snapshot_tag_auditor::SnapshotTagAuditor;
use crate::constants::SNAPSHOT_TAG_RETENTION;

/// JetStream stream names may not contain `.` (reserved for the subject
/// hierarchy), so the stream is named after the subject prefix with `_`.
pub(super) fn snapshot_tag_audit_stream_name() -> String {
    SNAPSHOT_TAG_AUDIT_STREAM.replace('.', "_")
}

pub struct JetStreamSnapshotTagAuditor {
    jetstream: jetstream::Context,
}

impl JetStreamSnapshotTagAuditor {
    pub async fn new(jetstream: jetstream::Context, num_replicas: usize) -> Result<Self, SnapshotTagAuditError> {
        jetstream
            .get_or_create_stream(stream::Config {
                name: snapshot_tag_audit_stream_name(),
                subjects: vec![format!("{SNAPSHOT_TAG_AUDIT_STREAM}.>")],
                retention: stream::RetentionPolicy::Limits,
                max_age: SNAPSHOT_TAG_RETENTION,
                num_replicas,
                storage: stream::StorageType::File,
                ..Default::default()
            })
            .await?;
        Ok(Self { jetstream })
    }
}

#[async_trait]
impl SnapshotTagAuditor for JetStreamSnapshotTagAuditor {
    async fn emit(&self, event: SnapshotTagAuditEvent) -> Result<(), SnapshotTagAuditError> {
        let subject = match &event {
            SnapshotTagAuditEvent::Set { tag, .. } | SnapshotTagAuditEvent::Cleared { tag, .. } => {
                snapshot_tag_audit_subject(&tag.tenant, &tag.name)
            }
        };
        let payload = serde_json::to_vec(&event)?;
        self.jetstream.publish(subject, Bytes::from(payload)).await?;
        Ok(())
    }
}
