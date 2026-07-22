//! §1892 Inspector de eventos de sesión.
//!
//! The append-only event log (§3) with its stable Event Contract (§301-337:
//! `event_id`, `session_id`, `seq`, `type`, `operation_id`, `correlation_id`,
//! `causation_id`, `idempotency_key`, `created_at`, `actor`) makes a session inspectable —
//! reconstructable, auditable, debuggable (§291). This module turns a replayed event log
//! into a flat, seq-ordered inspection over exactly those contract fields, plus an
//! operation grouping so a debugger can follow one logical operation across its events.
//! Pure + deterministic (no I/O).

use std::collections::BTreeMap;

use trogonai_session_contracts::SessionEvent;

use crate::kernel::event_payload_name;

/// One inspected event — the stable Event Contract fields (§301-337).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventInspection {
    pub seq: u64,
    pub event_id: String,
    pub event_type: &'static str,
    pub operation_id: String,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub actor_type: String,
    pub actor_id: String,
}

/// Inspect a replayed event log: every event mapped to its Event Contract fields, ordered
/// by `seq` (§339 ordering depends on `seq`). Deterministic.
pub fn inspect_events(events: &[SessionEvent]) -> Vec<EventInspection> {
    let mut inspected: Vec<EventInspection> = events
        .iter()
        .map(|event| {
            let (actor_type, actor_id) = event
                .actor
                .as_option()
                .map(|actor| (format!("{:?}", actor.r#type.as_known()), actor.id.clone()))
                .unwrap_or_default();
            EventInspection {
                seq: event.seq,
                event_id: event.event_id.clone(),
                event_type: event_payload_name(event),
                operation_id: event.operation_id.clone(),
                correlation_id: event.correlation_id.clone(),
                causation_id: event.causation_id.clone(),
                actor_type,
                actor_id,
            }
        })
        .collect();
    inspected.sort_by_key(|entry| entry.seq);
    inspected
}

/// Group inspected events by `operation_id` so a debugger can follow one logical operation
/// (prompt turn, switch, …) across its events (§332 `operation_id`). Keys are BTree-ordered
/// for deterministic output; each value stays seq-ordered.
pub fn inspect_by_operation(events: &[SessionEvent]) -> BTreeMap<String, Vec<EventInspection>> {
    let mut by_op: BTreeMap<String, Vec<EventInspection>> = BTreeMap::new();
    for entry in inspect_events(events) {
        by_op.entry(entry.operation_id.clone()).or_default().push(entry);
    }
    by_op
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::EnumValue;
    use buffa::MessageField;
    use trogonai_session_contracts::{
        Actor, ActorType, ModelSwitchedPayload, SessionCreatedPayload, SessionEventPayload,
    };

    fn event(seq: u64, op: &str, kind: trogonai_session_contracts::session_event_payload::Kind) -> SessionEvent {
        SessionEvent {
            event_id: format!("evt_{seq}"),
            session_id: "sess".to_string(),
            seq,
            operation_id: op.to_string(),
            correlation_id: format!("corr_{op}"),
            actor: MessageField::some(Actor {
                r#type: EnumValue::Known(ActorType::Kernel),
                id: "session-kernel".to_string(),
                ..Actor::default()
            }),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(kind),
                ..SessionEventPayload::default()
            }),
            ..SessionEvent::default()
        }
    }

    #[test]
    fn inspect_orders_by_seq_and_names_types() {
        use trogonai_session_contracts::session_event_payload::Kind;
        let events = vec![
            event(
                2,
                "op_switch",
                Kind::ModelSwitched(Box::new(ModelSwitchedPayload::default())),
            ),
            event(
                1,
                "op_create",
                Kind::SessionCreated(Box::new(SessionCreatedPayload::default())),
            ),
        ];
        let inspected = inspect_events(&events);
        assert_eq!(inspected[0].seq, 1);
        assert_eq!(inspected[0].event_type, "session_created");
        assert_eq!(inspected[1].seq, 2);
        assert_eq!(inspected[1].event_type, "model_switched");
        assert_eq!(inspected[1].actor_id, "session-kernel");
    }

    #[test]
    fn inspect_by_operation_groups_events() {
        use trogonai_session_contracts::session_event_payload::Kind;
        let events = vec![
            event(
                1,
                "op_create",
                Kind::SessionCreated(Box::new(SessionCreatedPayload::default())),
            ),
            event(
                2,
                "op_switch",
                Kind::ModelSwitched(Box::new(ModelSwitchedPayload::default())),
            ),
        ];
        let grouped = inspect_by_operation(&events);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped["op_create"][0].event_type, "session_created");
        assert_eq!(grouped["op_switch"][0].event_type, "model_switched");
    }
}
