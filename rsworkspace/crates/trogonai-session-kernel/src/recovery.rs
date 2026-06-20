use trogonai_session_contracts::{SessionEvent, SessionId, SessionSnapshot};

use crate::error::SessionKernelError;
use crate::event_log::EventLogBackend;
use crate::materialize::materialize_from_events;
use crate::state::RecoveryState;

/// Result of reconstructing in-memory session state from snapshots and replay.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredSession {
    pub session_id: SessionId,
    pub snapshot: SessionSnapshot,
    pub replayed_events: usize,
    pub state: RecoveryState,
}

pub trait EventLogReader: Send + Sync + Clone + 'static {
    fn read_session_events(
        &self,
        session_id: &SessionId,
    ) -> impl std::future::Future<Output = Result<Vec<SessionEvent>, SessionKernelError>> + Send;
}

pub trait SnapshotReader: Send + Sync + Clone + 'static {
    fn load_snapshot(
        &self,
        session_id: &SessionId,
    ) -> impl std::future::Future<Output = Result<Option<SessionSnapshot>, SessionKernelError>> + Send;
}

impl<T: EventLogBackend> EventLogReader for T {
    async fn read_session_events(&self, session_id: &SessionId) -> Result<Vec<SessionEvent>, SessionKernelError> {
        EventLogBackend::read_session_events(self, session_id).await
    }
}

pub async fn recover_session<E, S>(
    event_log: &E,
    snapshots: &S,
    session_id: &SessionId,
) -> Result<RecoveredSession, SessionKernelError>
where
    E: EventLogReader,
    S: SnapshotReader,
{
    let existing = snapshots.load_snapshot(session_id).await?;
    let last_applied_seq = existing.as_ref().map(|snapshot| snapshot.last_applied_seq).unwrap_or(0);
    let events = event_log.read_session_events(session_id).await?;
    let replay_events: Vec<_> = events
        .iter()
        .filter(|event| event.seq > last_applied_seq)
        .cloned()
        .collect();

    let snapshot = materialize_from_events(session_id.as_str(), &events, existing)?;
    let state = if snapshot.last_applied_seq < events.iter().map(|event| event.seq).max().unwrap_or(0) {
        RecoveryState::StaleSnapshot
    } else {
        RecoveryState::Completed
    };

    Ok(RecoveredSession {
        session_id: session_id.clone(),
        snapshot,
        replayed_events: replay_events.len(),
        state,
    })
}

#[cfg(test)]
mod tests {
    use buffa::{EnumValue, MessageField};
    use trogonai_session_contracts::{
        Actor, ActorType, SCHEMA_VERSION_V1, SessionCreatedPayload, SessionEvent, SessionEventPayload,
    };

    use super::*;
    use crate::event_log::InMemoryEventLog;

    #[derive(Clone)]
    struct MockSnapshotReader {
        snapshot: Option<SessionSnapshot>,
    }

    impl SnapshotReader for MockSnapshotReader {
        async fn load_snapshot(&self, _session_id: &SessionId) -> Result<Option<SessionSnapshot>, SessionKernelError> {
            Ok(self.snapshot.clone())
        }
    }

    fn created_event(session_id: &str, seq: u64) -> SessionEvent {
        SessionEvent {
            schema_version: SCHEMA_VERSION_V1,
            event_id: format!("evt_{seq}"),
            session_id: session_id.to_string(),
            seq,
            operation_id: "op_create".to_string(),
            correlation_id: "corr_create".to_string(),
            idempotency_key: format!("idem_{seq}"),
            actor: MessageField::some(Actor {
                r#type: EnumValue::Known(ActorType::Kernel),
                id: "session-kernel".to_string(),
                ..Actor::default()
            }),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(
                    SessionCreatedPayload {
                        title: "Recovery".to_string(),
                        cwd: "/repo".to_string(),
                        ..SessionCreatedPayload::default()
                    }
                    .into(),
                ),
                ..SessionEventPayload::default()
            }),
            ..SessionEvent::default()
        }
    }

    #[tokio::test]
    async fn recovery_replays_events_after_snapshot_seq() {
        let session_id = SessionId::new("sess_recovery").unwrap();
        let event_log = InMemoryEventLog::new();
        event_log.append(created_event("sess_recovery", 1)).await.unwrap();
        event_log.append(created_event("sess_recovery", 2)).await.unwrap();

        let recovered = recover_session(&event_log, &MockSnapshotReader { snapshot: None }, &session_id)
            .await
            .unwrap();

        assert_eq!(recovered.replayed_events, 2);
        assert_eq!(recovered.state, RecoveryState::Completed);
        assert_eq!(recovered.snapshot.last_applied_seq, 2);
    }
}
