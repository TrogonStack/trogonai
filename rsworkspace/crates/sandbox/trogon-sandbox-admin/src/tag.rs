use std::sync::Arc;
use std::time::SystemTime;

use async_nats::jetstream;
use trogon_sandbox_broker::snapshot_tag::{
    JetStreamSnapshotTagAuditor, NatsKvSnapshotTagStore, SnapshotTagStore, SnapshotTagStoreError,
    read_tag_audit_history,
};
use trogon_sandbox_contract::{
    ArtifactType, Digest, ServiceAccountId, SnapshotTag, SnapshotTagAuditEvent, SnapshotTagName, SnapshotTagRecord,
    TenantId,
};

use crate::constants::{ADMIN_SERVICE_ACCOUNT, TAG_AUDIT_JETSTREAM_REPLICAS, TAG_MUTATION_MAX_RETRIES};
use crate::{AdminError, Console};

async fn open_tag_store(js: jetstream::Context) -> Result<NatsKvSnapshotTagStore, AdminError> {
    let audit = JetStreamSnapshotTagAuditor::new(js.clone(), TAG_AUDIT_JETSTREAM_REPLICAS)
        .await
        .map_err(AdminError::step("open snapshot tag audit stream"))?;
    NatsKvSnapshotTagStore::new(js, Arc::new(audit))
        .await
        .map_err(AdminError::step("open snapshot tag store"))
}

fn not_found(tag: &SnapshotTag) -> AdminError {
    AdminError::TagNotFound {
        tenant: tag.tenant.to_string(),
        name: tag.name.to_string(),
    }
}

pub async fn set(
    js: jetstream::Context,
    tag: SnapshotTag,
    digest: Digest,
    artifact_type: ArtifactType,
    expected_revision: Option<u64>,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let store = open_tag_store(js).await?;
    let record = SnapshotTagRecord {
        digest,
        artifact_type,
        set_by: ServiceAccountId::new(ADMIN_SERVICE_ACCOUNT),
        set_at: SystemTime::now(),
        retention: None,
    };

    for attempt in 1..=TAG_MUTATION_MAX_RETRIES {
        match store.set(&tag, record.clone(), expected_revision).await {
            Ok(revision) => return console.out(revision),
            // Without an expected revision there is nothing for the operator to refresh, so the
            // first conflict is final.
            Err(SnapshotTagStoreError::RevisionConflict)
                if expected_revision.is_some() && attempt < TAG_MUTATION_MAX_RETRIES => {}
            Err(SnapshotTagStoreError::RevisionConflict) if expected_revision.is_some() => {
                return Err(AdminError::TagRevisionConflict {
                    attempts: TAG_MUTATION_MAX_RETRIES,
                });
            }
            Err(error) => return Err(AdminError::step("set tag")(error)),
        }
    }
    Err(AdminError::TagRevisionConflict {
        attempts: TAG_MUTATION_MAX_RETRIES,
    })
}

pub async fn list(js: jetstream::Context, tenant: &TenantId, console: &mut Console<'_>) -> Result<(), AdminError> {
    let rows = open_tag_store(js)
        .await?
        .list(tenant)
        .await
        .map_err(AdminError::step("list tags"))?;
    if rows.is_empty() {
        return console.out(format_args!("(no tags for tenant {tenant})"));
    }
    for (tag, record) in rows {
        console.out(format_args!(
            "{} {} {:?} {:?}",
            tag.name, record.digest, record.artifact_type, record.set_at
        ))?;
    }
    Ok(())
}

pub async fn get(js: jetstream::Context, tag: &SnapshotTag, console: &mut Console<'_>) -> Result<(), AdminError> {
    let record = open_tag_store(js)
        .await?
        .get(tag)
        .await
        .map_err(AdminError::step("read tag"))?
        .ok_or_else(|| not_found(tag))?;
    console.out(format_args!(
        "{} {} {:?} set_by={} set_at={:?} retention={:?}",
        tag.name, record.digest, record.artifact_type, record.set_by, record.set_at, record.retention
    ))
}

pub async fn history(
    js: jetstream::Context,
    tenant: &TenantId,
    name: &SnapshotTagName,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let events = read_tag_audit_history(js, tenant, name)
        .await
        .map_err(AdminError::step("read tag audit history"))?;
    if events.is_empty() {
        return console.out(format_args!("(no audit history for {tenant}/{name})"));
    }
    for event in events {
        match event {
            SnapshotTagAuditEvent::Set { prev, next, by, at, .. } => {
                let prev_label = prev.map_or_else(|| "(none)".to_string(), |digest| digest.to_string());
                console.out(format_args!("set {prev_label} -> {next} by {by} at {at:?}"))?;
            }
            SnapshotTagAuditEvent::Cleared { prev, by, at, .. } => {
                console.out(format_args!("cleared {prev} by {by} at {at:?}"))?;
            }
        }
    }
    Ok(())
}

pub async fn clear(js: jetstream::Context, tag: &SnapshotTag, console: &mut Console<'_>) -> Result<(), AdminError> {
    let store = open_tag_store(js).await?;
    for attempt in 1..=TAG_MUTATION_MAX_RETRIES {
        let Some((_, revision)) = store.get_entry(tag).await.map_err(AdminError::step("read tag"))? else {
            return Err(not_found(tag));
        };
        match store.clear(tag, revision).await {
            Ok(()) => return console.err(format_args!("cleared tag {}/{}", tag.tenant, tag.name)),
            Err(SnapshotTagStoreError::RevisionConflict) if attempt < TAG_MUTATION_MAX_RETRIES => {}
            Err(SnapshotTagStoreError::NotFound) => return Err(not_found(tag)),
            Err(error) => return Err(AdminError::step("clear tag")(error)),
        }
    }
    Err(AdminError::TagClearExhausted {
        attempts: TAG_MUTATION_MAX_RETRIES,
    })
}
