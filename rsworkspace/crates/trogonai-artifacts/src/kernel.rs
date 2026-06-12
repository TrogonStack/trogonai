use std::collections::HashSet;

use buffa::{EnumValue, MessageField};
use trogon_nats::jetstream::ObjectStoreDelete;
use trogonai_session_contracts::{
    Actor, ArtifactCreatedPayload, ArtifactGcDeletedPayload, ArtifactGcMarkedPayload,
    ArtifactMetadata, ArtifactRetentionPolicy, CorrelationId, EventId, IdempotencyKey, OperationId,
    RedactionAppliedPayload, SCHEMA_VERSION_V1, SessionEvent, SessionEventPayload,
    SessionSnapshotState, __buffa::oneof::tool_call_result::Kind as ToolResultKind,
};

use crate::error::ArtifactStoreError;
use crate::redaction::{redact_secrets, RedactionOutcome};
use crate::store::{ArtifactStore, StoreArtifactRequest, StoredArtifact};
use trogonai_session_kernel::{EventLogBackend, SessionKernel, SessionLeaseFactory};

/// Context required to append an `artifact_created` session event.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactEventContext {
    pub operation_id: OperationId,
    pub correlation_id: CorrelationId,
    pub idempotency_key: IdempotencyKey,
    pub causation_id: Option<EventId>,
    pub actor: Actor,
}

/// Persists artifact content and emits `artifact_created` through the session kernel.
pub async fn store_and_emit_artifact_created<E, Kv, L, Os>(
    kernel: &SessionKernel<E, Kv, L>,
    store: &ArtifactStore<Os>,
    request: StoreArtifactRequest,
    context: ArtifactEventContext,
) -> Result<(StoredArtifact, SessionEvent), ArtifactStoreError>
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
    Os: trogon_nats::jetstream::ObjectStorePut
        + trogon_nats::jetstream::ObjectStoreGet
        + Clone
        + Send
        + Sync
        + 'static,
{
    let mut stored = store.store(request).await?;

    // Detect secrets in the preview before persisting the metadata, so they never
    // surface in previews/exports (§ Security). When something is redacted, record
    // a `redaction_applied` event for audit.
    let redaction = redact_secrets(&stored.metadata.preview);
    if redaction.redacted() {
        stored.metadata.preview = redaction.text.clone();
        let redaction_event = redaction_applied_event(&stored, &context, &redaction);
        kernel.append_event(redaction_event).await?;
    }

    let event = artifact_created_event(&stored, &context);
    let appended = kernel.append_event(event).await?;
    Ok((stored, appended))
}

fn redaction_applied_event(
    stored: &StoredArtifact,
    context: &ArtifactEventContext,
    redaction: &RedactionOutcome,
) -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: format!("evt_redact_{}", uuid::Uuid::now_v7()),
        session_id: stored.metadata.session_id.clone(),
        seq: 0,
        operation_id: context.operation_id.as_str().to_string(),
        correlation_id: context.correlation_id.as_str().to_string(),
        causation_id: context.causation_id.as_ref().map(|id| id.as_str().to_string()),
        idempotency_key: format!("{}_redaction", context.idempotency_key.as_str()),
        created_at: stored.metadata.created_at.clone(),
        actor: MessageField::some(context.actor.clone()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                RedactionAppliedPayload {
                    target_ref: stored.metadata.artifact_id.clone(),
                    redaction_kind: redaction.kinds.join(","),
                    occurrences: redaction.occurrences,
                    ..RedactionAppliedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

fn artifact_created_event(stored: &StoredArtifact, context: &ArtifactEventContext) -> SessionEvent {
    let session_id = stored.metadata.session_id.clone();
    let event_id = format!("evt_{}", uuid::Uuid::now_v7());

    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id,
        session_id,
        seq: 0,
        operation_id: context.operation_id.as_str().to_string(),
        correlation_id: context.correlation_id.as_str().to_string(),
        causation_id: context.causation_id.as_ref().map(|id| id.as_str().to_string()),
        idempotency_key: context.idempotency_key.as_str().to_string(),
        created_at: stored.metadata.created_at.clone(),
        actor: MessageField::some(context.actor.clone()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                ArtifactCreatedPayload {
                    artifact: MessageField::some(stored.metadata.clone()),
                    ..ArtifactCreatedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

/// Builds an `artifact_created` event for an already persisted artifact.
pub fn build_artifact_created_event(
    stored: &StoredArtifact,
    context: &ArtifactEventContext,
) -> SessionEvent {
    artifact_created_event(stored, context)
}

/// Garbage-collect unreferenced, ephemeral artifacts for a session
/// (§ Event Log Compaction and Retention). Referenced artifacts are never deleted;
/// only artifacts with `EPHEMERAL` retention that no tool result still points to are
/// marked (`artifact_gc_marked`), physically removed from the object store, then
/// recorded as deleted (`artifact_gc_deleted`). Returns the deleted artifact ids.
pub async fn gc_unreferenced_artifacts<E, Kv, L, Os>(
    kernel: &SessionKernel<E, Kv, L>,
    store: &ArtifactStore<Os>,
    session_state: &SessionSnapshotState,
    context: &ArtifactEventContext,
) -> Result<Vec<String>, ArtifactStoreError>
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
    let referenced = referenced_artifact_ids(session_state);
    let mut deleted = Vec::new();
    for artifact in &session_state.artifacts {
        let ephemeral =
            artifact.retention_policy == EnumValue::Known(ArtifactRetentionPolicy::Ephemeral);
        // Referenced or retained artifacts are kept; only unreferenced ephemerals GC.
        if referenced.contains(&artifact.artifact_id) || !ephemeral {
            continue;
        }
        kernel.append_event(artifact_gc_marked_event(artifact, context)).await?;
        store.delete(artifact).await?;
        kernel.append_event(artifact_gc_deleted_event(artifact, context)).await?;
        deleted.push(artifact.artifact_id.clone());
    }
    Ok(deleted)
}

fn referenced_artifact_ids(state: &SessionSnapshotState) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for tool in &state.tool_calls {
        if let Some(result) = tool.result.as_option()
            && let Some(ToolResultKind::ArtifactRef(art_ref)) = result.kind.as_ref()
        {
            referenced.insert(art_ref.artifact_id.clone());
        }
    }
    referenced
}

fn artifact_gc_marked_event(
    artifact: &ArtifactMetadata,
    context: &ArtifactEventContext,
) -> SessionEvent {
    gc_event(
        artifact,
        context,
        "gcmarked",
        ArtifactGcMarkedPayload {
            artifact_id: artifact.artifact_id.clone(),
            ..ArtifactGcMarkedPayload::default()
        }
        .into(),
    )
}

fn artifact_gc_deleted_event(
    artifact: &ArtifactMetadata,
    context: &ArtifactEventContext,
) -> SessionEvent {
    gc_event(
        artifact,
        context,
        "gcdeleted",
        ArtifactGcDeletedPayload {
            artifact_id: artifact.artifact_id.clone(),
            ..ArtifactGcDeletedPayload::default()
        }
        .into(),
    )
}

fn gc_event(
    artifact: &ArtifactMetadata,
    context: &ArtifactEventContext,
    suffix: &str,
    kind: trogonai_session_contracts::session_event_payload::Kind,
) -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: format!("evt_{suffix}_{}", artifact.artifact_id),
        session_id: artifact.session_id.clone(),
        seq: 0,
        operation_id: context.operation_id.as_str().to_string(),
        correlation_id: context.correlation_id.as_str().to_string(),
        causation_id: context.causation_id.as_ref().map(|id| id.as_str().to_string()),
        idempotency_key: format!("{}_{suffix}_{}", context.idempotency_key.as_str(), artifact.artifact_id),
        created_at: artifact.created_at.clone(),
        actor: MessageField::some(context.actor.clone()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(kind),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::EnumValue;
    use bytes::Bytes;
    use trogon_nats::jetstream::MockObjectStore;
    use trogonai_session_contracts::{ActorType, EventId, SessionId, session_event_payload};
    use trogonai_session_kernel::{
        InMemoryEventLog, MockSessionLease, MockSessionLeaseFactory, SessionKernelConfig,
        SessionLeaseManager, SnapshotStore,
    };

    use crate::config::ArtifactStoreConfig;
    use crate::store::ArtifactStorageMode;

    #[tokio::test]
    async fn store_and_emit_appends_artifact_created_event() {
        let session_id = SessionId::new("sess_emit").unwrap();
        let event_id = EventId::new("evt_tool_done").unwrap();
        let config = ArtifactStoreConfig {
            inline_limit_bytes: 1024,
            preview_max_bytes: 64,
            bucket_name: "ACP_SESSION_ARTIFACTS".to_string(),
            permission_scope: "workspace:default".to_string(),
        };
        let store = ArtifactStore::new(MockObjectStore::new(), config);
        let snapshot_store = trogon_nats::jetstream::MockJetStreamKvStore::new();
        snapshot_store.enqueue_get_none();
        let kernel = SessionKernel::new(
            SessionKernelConfig::default(),
            InMemoryEventLog::new(),
            SnapshotStore::new(snapshot_store, SessionKernelConfig::default()),
            SessionLeaseManager::new(MockSessionLeaseFactory::new(MockSessionLease::new()), "node-1"),
        );

        let (stored, appended) = store_and_emit_artifact_created(
            &kernel,
            &store,
            StoreArtifactRequest::new(
                session_id.clone(),
                event_id,
                "text/plain",
                Bytes::from_static(b"tool output"),
            ),
            ArtifactEventContext {
                operation_id: OperationId::new("op_tool").unwrap(),
                correlation_id: CorrelationId::new("corr_tool").unwrap(),
                idempotency_key: IdempotencyKey::new("idem_artifact").unwrap(),
                causation_id: None,
                actor: Actor {
                    r#type: EnumValue::Known(ActorType::Kernel),
                    id: "session-kernel".to_string(),
                    ..Actor::default()
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(stored.storage_mode, ArtifactStorageMode::Inline);
        assert_eq!(appended.seq, 1);
        let payload = appended.payload.as_option().unwrap();
        match payload.kind.as_ref().unwrap() {
            session_event_payload::Kind::ArtifactCreated(created) => {
                let artifact = created.artifact.as_option().unwrap();
                assert_eq!(artifact.artifact_id, stored.metadata.artifact_id);
                assert_eq!(artifact.session_id, session_id.as_str());
            }
            other => panic!("expected artifact_created, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn store_and_emit_redacts_secrets_and_emits_redaction_applied() {
        let session_id = SessionId::new("sess_secret").unwrap();
        let event_id = EventId::new("evt_tool_secret").unwrap();
        let config = ArtifactStoreConfig {
            inline_limit_bytes: 1024,
            preview_max_bytes: 256,
            bucket_name: "ACP_SESSION_ARTIFACTS".to_string(),
            permission_scope: "workspace:default".to_string(),
        };
        let store = ArtifactStore::new(MockObjectStore::new(), config);
        let snapshot_store = trogon_nats::jetstream::MockJetStreamKvStore::new();
        snapshot_store.enqueue_get_none();
        snapshot_store.enqueue_get_none();
        let event_log = InMemoryEventLog::new();
        let kernel = SessionKernel::new(
            SessionKernelConfig::default(),
            event_log.clone(),
            SnapshotStore::new(snapshot_store, SessionKernelConfig::default()),
            SessionLeaseManager::new(MockSessionLeaseFactory::new(MockSessionLease::new()), "node-1"),
        );

        let (stored, _appended) = store_and_emit_artifact_created(
            &kernel,
            &store,
            StoreArtifactRequest::new(
                session_id.clone(),
                event_id,
                "text/plain",
                Bytes::from_static(b"export AWS_KEY=AKIAIOSFODNN7EXAMPLE before deploy"),
            ),
            ArtifactEventContext {
                operation_id: OperationId::new("op_tool").unwrap(),
                correlation_id: CorrelationId::new("corr_tool").unwrap(),
                idempotency_key: IdempotencyKey::new("idem_secret").unwrap(),
                causation_id: None,
                actor: Actor {
                    r#type: EnumValue::Known(ActorType::Kernel),
                    id: "session-kernel".to_string(),
                    ..Actor::default()
                },
            },
        )
        .await
        .unwrap();

        // The secret never appears in the persisted preview.
        assert!(stored.metadata.preview.contains("[REDACTED:aws_access_key]"));
        assert!(!stored.metadata.preview.contains("AKIAIOSFODNN7EXAMPLE"));

        // A redaction_applied event was recorded alongside artifact_created.
        let events = event_log.read_session_events(&session_id).await.unwrap();
        let has_redaction = events.iter().any(|event| {
            matches!(
                event.payload.as_option().and_then(|p| p.kind.as_ref()),
                Some(session_event_payload::Kind::RedactionApplied(_))
            )
        });
        assert!(has_redaction, "expected a redaction_applied event");
    }

    #[tokio::test]
    async fn gc_removes_unreferenced_ephemeral_artifacts_only() {
        use trogonai_session_contracts::{
            ArtifactRef, CanonicalToolCall, ToolCallResult,
        };

        let bucket = "ACP_SESSION_ARTIFACTS";
        let object_store = MockObjectStore::new();
        object_store.seed("sessions/sess_gc/art_eph", Bytes::from_static(b"ephemeral"));
        object_store.seed("sessions/sess_gc/art_ref", Bytes::from_static(b"referenced"));
        let store = ArtifactStore::new(
            object_store.clone(),
            ArtifactStoreConfig {
                inline_limit_bytes: 1024,
                preview_max_bytes: 64,
                bucket_name: bucket.to_string(),
                permission_scope: "workspace:default".to_string(),
            },
        );
        let snapshot_store = trogon_nats::jetstream::MockJetStreamKvStore::new();
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
            session_id: "sess_gc".to_string(),
            storage_ref: format!("obj://{bucket}/sessions/sess_gc/{id}"),
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
            operation_id: OperationId::new("op_gc").unwrap(),
            correlation_id: CorrelationId::new("corr_gc").unwrap(),
            idempotency_key: IdempotencyKey::new("idem_gc").unwrap(),
            causation_id: None,
            actor: Actor {
                r#type: EnumValue::Known(ActorType::Kernel),
                id: "session-kernel".to_string(),
                ..Actor::default()
            },
        };

        let deleted = gc_unreferenced_artifacts(&kernel, &store, &state, &context)
            .await
            .unwrap();
        // Only the unreferenced ephemeral artifact is collected.
        assert_eq!(deleted, vec!["art_eph".to_string()]);

        // Its blob is physically gone; the referenced one remains.
        let keys: Vec<String> = object_store.stored_objects().into_iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&"sessions/sess_gc/art_ref".to_string()));
        assert!(!keys.contains(&"sessions/sess_gc/art_eph".to_string()));

        // Both GC audit events are recorded.
        let events = event_log
            .read_session_events(&SessionId::new("sess_gc").unwrap())
            .await
            .unwrap();
        let has = |pred: fn(&session_event_payload::Kind) -> bool| {
            events.iter().any(|e| {
                e.payload.as_option().and_then(|p| p.kind.as_ref()).is_some_and(pred)
            })
        };
        assert!(has(|k| matches!(k, session_event_payload::Kind::ArtifactGcMarked(_))));
        assert!(has(|k| matches!(k, session_event_payload::Kind::ArtifactGcDeleted(_))));
    }
}
