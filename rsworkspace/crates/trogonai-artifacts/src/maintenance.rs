//! Per-session Event Log Compaction & Retention maintenance pass
//! (§ Event Log Compaction and Retention).
//!
//! Composes the two retention actions the doc lists into one production-callable unit:
//! archive events older than the retention cutoff (`events_archived`) and garbage-collect
//! unreferenced ephemeral artifacts (`artifact_gc_marked` / `artifact_gc_deleted`).
//! Referenced and non-ephemeral artifacts are never deleted; archival is a no-op when no
//! event is old enough, so the pass is safe to run repeatedly.

use buffa_types::google::protobuf::Timestamp;
use trogon_nats::jetstream::ObjectStoreDelete;
use trogonai_session_contracts::{SessionId, SessionSnapshotState};
use trogonai_session_kernel::{EventLogBackend, SessionKernel, SessionLeaseFactory};

use crate::error::ArtifactStoreError;
use crate::kernel::{ArtifactEventContext, gc_unreferenced_artifacts};
use crate::store::ArtifactStore;

/// Outcome of one session maintenance pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionMaintenanceReport {
    /// Events archived through the retention watermark (`events_archived`).
    pub archived_count: u64,
    /// Artifact ids garbage-collected (`artifact_gc_deleted`).
    pub gc_deleted: Vec<String>,
}

/// Run one Event Log Compaction & Retention pass for a session.
///
/// `retention_cutoff` is the oldest timestamp to keep unarchived (e.g. `now - retention`);
/// `now` stamps the emitted maintenance events. `session_state` is the materialized
/// snapshot whose tool-call references decide which ephemeral artifacts are unreferenced.
pub async fn run_session_maintenance<E, Kv, L, Os>(
    kernel: &SessionKernel<E, Kv, L>,
    store: &ArtifactStore<Os>,
    session_id: &SessionId,
    session_state: &SessionSnapshotState,
    retention_cutoff: Timestamp,
    now: Timestamp,
    context: &ArtifactEventContext,
) -> Result<SessionMaintenanceReport, ArtifactStoreError>
where
    E: EventLogBackend,
    Kv: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    L: SessionLeaseFactory + Clone + 'static,
    Os: ObjectStoreDelete + Clone + Send + Sync + 'static,
{
    // Archive old events first (retention watermark), then GC unreferenced ephemerals.
    let archived_count = kernel
        .archive_events_older_than(
            session_id,
            retention_cutoff,
            context.operation_id.as_str(),
            context.actor.clone(),
            now,
        )
        .await?;
    let gc_deleted = gc_unreferenced_artifacts(kernel, store, session_state, context).await?;
    Ok(SessionMaintenanceReport {
        archived_count,
        gc_deleted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::{EnumValue, MessageField};
    use bytes::Bytes;
    use trogon_nats::jetstream::{MockJetStreamKvStore, MockObjectStore};
    use trogonai_session_contracts::{
        Actor, ActorType, ArtifactMetadata, ArtifactRef, ArtifactRetentionPolicy, CanonicalToolCall, CorrelationId,
        IdempotencyKey, OperationId, ToolCallResult, session_event_payload,
    };
    use trogonai_session_kernel::{
        InMemoryEventLog, MockSessionLease, MockSessionLeaseFactory, SessionKernelConfig, SessionLeaseManager,
        SnapshotStore,
    };

    use crate::config::ArtifactStoreConfig;

    // § Event Log Compaction and Retention: one pass composes archival (a no-op on an
    // empty log) with GC of unreferenced ephemeral artifacts, reporting both.
    #[tokio::test]
    async fn maintenance_pass_gcs_unreferenced_ephemeral_and_reports() {
        let bucket = "ACP_SESSION_ARTIFACTS";
        let object_store = MockObjectStore::new();
        object_store.seed("sessions/sess_maint/art_eph", Bytes::from_static(b"ephemeral"));
        object_store.seed("sessions/sess_maint/art_ref", Bytes::from_static(b"referenced"));
        let store = ArtifactStore::new(
            object_store.clone(),
            ArtifactStoreConfig {
                inline_limit_bytes: 1024,
                preview_max_bytes: 64,
                bucket_name: bucket.to_string(),
                permission_scope: "workspace:default".to_string(),
            },
        );
        let snapshot_store = MockJetStreamKvStore::new();
        snapshot_store.enqueue_get_none();
        snapshot_store.enqueue_get_none();
        let event_log = InMemoryEventLog::new();
        let kernel = SessionKernel::new(
            SessionKernelConfig::default(),
            event_log.clone(),
            SnapshotStore::new(snapshot_store, SessionKernelConfig::default()),
            SessionLeaseManager::new(MockSessionLeaseFactory::new(MockSessionLease::new()), "node-1"),
        );

        let metadata = |id: &str| ArtifactMetadata {
            artifact_id: id.to_string(),
            session_id: "sess_maint".to_string(),
            storage_ref: format!("obj://{bucket}/sessions/sess_maint/{id}"),
            retention_policy: EnumValue::Known(ArtifactRetentionPolicy::Ephemeral),
            ..ArtifactMetadata::default()
        };
        let state = SessionSnapshotState {
            artifacts: vec![metadata("art_eph"), metadata("art_ref")],
            tool_calls: vec![CanonicalToolCall {
                result: MessageField::some(ToolCallResult {
                    kind: Some(
                        ArtifactRef {
                            artifact_id: "art_ref".to_string(),
                            ..ArtifactRef::default()
                        }
                        .into(),
                    ),
                    ..ToolCallResult::default()
                }),
                ..CanonicalToolCall::default()
            }],
            ..SessionSnapshotState::default()
        };
        let context = ArtifactEventContext {
            operation_id: OperationId::new("op_maint").unwrap(),
            correlation_id: CorrelationId::new("corr_maint").unwrap(),
            idempotency_key: IdempotencyKey::new("idem_maint").unwrap(),
            causation_id: None,
            actor: Actor {
                r#type: EnumValue::Known(ActorType::Kernel),
                id: "session-kernel".to_string(),
                ..Actor::default()
            },
        };

        let session_id = SessionId::new("sess_maint").unwrap();
        let report = run_session_maintenance(
            &kernel,
            &store,
            &session_id,
            &state,
            Timestamp::default(),
            Timestamp::default(),
            &context,
        )
        .await
        .unwrap();

        // Empty event log -> nothing within the retention cutoff -> no-op archive.
        assert_eq!(report.archived_count, 0);
        // Only the unreferenced ephemeral artifact is collected.
        assert_eq!(report.gc_deleted, vec!["art_eph".to_string()]);

        // Its blob is physically gone; the referenced one remains.
        let keys: Vec<String> = object_store.stored_objects().into_iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&"sessions/sess_maint/art_ref".to_string()));
        assert!(!keys.contains(&"sessions/sess_maint/art_eph".to_string()));

        // GC audit events were recorded.
        let events = event_log.read_session_events(&session_id).await.unwrap();
        let has = |pred: fn(&session_event_payload::Kind) -> bool| {
            events
                .iter()
                .any(|e| e.payload.as_option().and_then(|p| p.kind.as_ref()).is_some_and(pred))
        };
        assert!(has(|k| matches!(k, session_event_payload::Kind::ArtifactGcDeleted(_))));
    }
}
