use async_nats::jetstream;
use futures::StreamExt;
use trogon_sandbox_contract::{SnapshotTagAuditEvent, SnapshotTagName, TenantId};
use trogon_sandbox_protocol::subjects::snapshot_tag_audit_subject;

use super::jet_stream_snapshot_tag_auditor::snapshot_tag_audit_stream_name;
use super::snapshot_tag_audit_error::SnapshotTagAuditError;
use crate::constants::SNAPSHOT_TAG_AUDIT_FETCH_LIMIT;

pub async fn read_tag_audit_history(
    jetstream: jetstream::Context,
    tenant: &TenantId,
    name: &SnapshotTagName,
) -> Result<Vec<SnapshotTagAuditEvent>, SnapshotTagAuditError> {
    let subject = snapshot_tag_audit_subject(tenant, name);
    let stream = jetstream.get_stream(snapshot_tag_audit_stream_name()).await?;

    let consumer = stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            filter_subject: subject.clone(),
            ..Default::default()
        })
        .await?;

    let mut messages = consumer
        .fetch()
        .max_messages(SNAPSHOT_TAG_AUDIT_FETCH_LIMIT)
        .messages()
        .await?;

    let mut events = Vec::new();
    while let Some(msg) = messages.next().await {
        let msg = msg?;
        let event: SnapshotTagAuditEvent = serde_json::from_slice(&msg.payload)?;
        events.push(event);
    }

    events.sort_by_key(|event| match event {
        SnapshotTagAuditEvent::Set { at, .. } | SnapshotTagAuditEvent::Cleared { at, .. } => *at,
    });
    Ok(events)
}
