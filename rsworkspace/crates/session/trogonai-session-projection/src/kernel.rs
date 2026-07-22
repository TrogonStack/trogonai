use trogonai_session_contracts::{ContextTwin, SessionId, SessionSnapshot};
use trogonai_session_kernel::{EventLogBackend, SessionKernel, SessionLeaseFactory};

use crate::context_twin::derive_context_twin;
use crate::error::ProjectionError;
use crate::event::context_twin_updated_event;
use crate::store::ContextTwinStore;
use crate::telemetry;
use crate::update_context::ContextTwinUpdateContext;

/// Derive Context Twin from canonical session state, persist to KV, and emit `context_twin_updated`.
pub async fn update_context_twin<E, SnapKv, LeaseF, TwinKv>(
    kernel: &SessionKernel<E, SnapKv, LeaseF>,
    twin_store: &ContextTwinStore<TwinKv>,
    context: ContextTwinUpdateContext,
) -> Result<(ContextTwin, trogonai_session_contracts::SessionEvent), ProjectionError>
where
    E: EventLogBackend,
    SnapKv: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    LeaseF: SessionLeaseFactory + Clone + 'static,
    TwinKv: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
{
    let session_id = context.session_id.clone();
    let updated_at = context.updated_at.clone();
    let snapshot = load_or_materialize(kernel, &session_id).await?;
    let state = snapshot
        .state
        .into_option()
        .ok_or(ProjectionError::MissingField("snapshot.state"))?;

    let context_twin = derive_context_twin(
        session_id.as_str(),
        &state,
        snapshot.last_applied_seq,
        updated_at,
    );

    twin_store.save_context_twin(&context_twin).await?;

    let event = context_twin_updated_event(&context, &context_twin);
    let appended = kernel.append_event(event).await?;
    let _ = kernel.materialize_state(&session_id).await?;

    telemetry::metrics::record_context_twin_updated(session_id.as_str(), context_twin.derived_from_seq);

    Ok((context_twin, appended))
}

async fn load_or_materialize<E, SnapKv, LeaseF>(
    kernel: &SessionKernel<E, SnapKv, LeaseF>,
    session_id: &SessionId,
) -> Result<SessionSnapshot, ProjectionError>
where
    E: EventLogBackend,
    SnapKv: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    LeaseF: SessionLeaseFactory + Clone + 'static,
{
    if let Some(snapshot) = kernel.load_snapshot(session_id).await? {
        return Ok(snapshot);
    }
    Ok(kernel.materialize_state(session_id).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::{EnumValue, MessageField};
    use trogonai_session_contracts::{
        ActorType, IdempotencyKey, OperationId, SessionCreatedPayload, SessionEventPayload,
        SCHEMA_VERSION_V1,
    };
    use trogonai_session_kernel::{
        InMemoryEventLog, SessionKernelConfig, SessionKvLeaseFactory, SessionLeaseManager,
        SnapshotStore,
    };

    fn timestamp() -> buffa_types::google::protobuf::Timestamp {
        buffa_types::google::protobuf::Timestamp {
            seconds: 1_748_995_200,
            nanos: 0,
            ..buffa_types::google::protobuf::Timestamp::default()
        }
    }

    fn created_event(session_id: &str) -> trogonai_session_contracts::SessionEvent {
        trogonai_session_contracts::SessionEvent {
            schema_version: SCHEMA_VERSION_V1,
            event_id: "evt_create".to_string(),
            session_id: session_id.to_string(),
            seq: 1,
            operation_id: "op_create".to_string(),
            correlation_id: "corr_create".to_string(),
            idempotency_key: "idem_create".to_string(),
            created_at: MessageField::some(timestamp()),
            actor: MessageField::some(trogonai_session_contracts::Actor {
                r#type: EnumValue::Known(ActorType::Kernel),
                id: "session-kernel".to_string(),
                ..trogonai_session_contracts::Actor::default()
            }),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(
                    SessionCreatedPayload {
                        title: "Projection".to_string(),
                        cwd: "/repo".to_string(),
                        ..SessionCreatedPayload::default()
                    }
                    .into(),
                ),
                ..SessionEventPayload::default()
            }),
            ..trogonai_session_contracts::SessionEvent::default()
        }
    }

    #[tokio::test]
    async fn update_context_twin_persists_and_appends_event() {
        let session_id = SessionId::new("sess_update_twin").unwrap();
        let event_log = InMemoryEventLog::default();
        event_log.append(created_event(session_id.as_str())).await.unwrap();

        let config = SessionKernelConfig::default();
        let projection_config = crate::config::ProjectionConfig::default();
        let snap_store = SnapshotStore::new(trogon_nats::jetstream::MockJetStreamKvStore::new(), config.clone());
        let twin_store = ContextTwinStore::new(
            trogon_nats::jetstream::MockJetStreamKvStore::new(),
            projection_config,
        );
        let leases = SessionLeaseManager::new(
            SessionKvLeaseFactory::new(trogon_nats::jetstream::MockJetStreamKvStore::new(), &config),
            "test-holder",
        );
        let kernel = SessionKernel::new(config, event_log.clone(), snap_store, leases);

        let _guard = kernel
            .acquire_session_lease(&session_id, trogonai_session_kernel::SessionMutatingOperation::Compact)
            .await
            .unwrap();

        let (twin, event) = update_context_twin(
            &kernel,
            &twin_store,
            ContextTwinUpdateContext {
                session_id: session_id.clone(),
                operation_id: OperationId::new("op_update_twin").unwrap(),
                correlation_id: "corr_update_twin".to_string(),
                idempotency_key: IdempotencyKey::new("idem_update_twin").unwrap(),
                causation_id: None,
                actor: trogonai_session_contracts::Actor {
                    r#type: EnumValue::Known(ActorType::Kernel),
                    id: "session-projection".to_string(),
                    ..trogonai_session_contracts::Actor::default()
                },
                updated_at: timestamp(),
            },
        )
        .await
        .unwrap();

        assert_eq!(twin.session_id, session_id.as_str());
        assert_eq!(event.session_id, session_id.as_str());
        assert_eq!(event.seq, 2);
        assert_eq!(twin.derived_from_seq, 1);
        assert_eq!(twin.current_objective, "Projection");
    }
}
