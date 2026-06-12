use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::{Actor, EventId, IdempotencyKey, OperationId, SessionId};

/// Event metadata required to append a `context_twin_updated` session event.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextTwinUpdateContext {
    pub session_id: SessionId,
    pub operation_id: OperationId,
    pub correlation_id: String,
    pub idempotency_key: IdempotencyKey,
    pub causation_id: Option<EventId>,
    pub actor: Actor,
    pub updated_at: Timestamp,
}
