use buffa::MessageField;
use trogonai_session_contracts::{
    ContextTwin, ContextTwinUpdatedPayload, SCHEMA_VERSION_V1, SessionEvent, SessionEventPayload,
};

use crate::update_context::ContextTwinUpdateContext;

pub(crate) fn context_twin_updated_event(
    context: &ContextTwinUpdateContext,
    context_twin: &ContextTwin,
) -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: format!("evt_{}", uuid::Uuid::now_v7()),
        session_id: context.session_id.as_str().to_string(),
        seq: 0,
        operation_id: context.operation_id.as_str().to_string(),
        correlation_id: context.correlation_id.clone(),
        causation_id: context.causation_id.as_ref().map(|id| id.as_str().to_string()),
        idempotency_key: context.idempotency_key.as_str().to_string(),
        created_at: MessageField::some(context.updated_at.clone()),
        actor: MessageField::some(context.actor.clone()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                ContextTwinUpdatedPayload {
                    context_twin: MessageField::some(context_twin.clone()),
                    ..ContextTwinUpdatedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}
