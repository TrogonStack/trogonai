use trogon_sandbox_contract::{EntityState, LeaseId, SandboxEntityRecord};

use crate::{EntityEvent, IllegalTransitionError};

/// The single entity transition table.
///
/// Returns the next state, or an error when the move is not allowed, including a lease that does not match the
/// bound one.
pub fn next_entity_state(state: &EntityState, event: &EntityEvent) -> Result<EntityState, IllegalTransitionError> {
    let bound = state.lease();
    let matches = |lease: &LeaseId| bound == Some(lease);
    let next = match (state, event) {
        (EntityState::Creating, EntityEvent::Provisioned) => Some(EntityState::Stopped),
        (EntityState::Stopped | EntityState::Failed { .. }, EntityEvent::StartRequested { lease }) => {
            Some(EntityState::Starting { lease: lease.clone() })
        }
        (EntityState::Starting { .. }, EntityEvent::LeaseReady { lease }) if matches(lease) => {
            Some(EntityState::Running { lease: lease.clone() })
        }
        (EntityState::Running { .. }, EntityEvent::Paused { lease }) if matches(lease) => {
            Some(EntityState::Paused { lease: lease.clone() })
        }
        (EntityState::Paused { .. }, EntityEvent::Resumed { lease }) if matches(lease) => {
            Some(EntityState::Running { lease: lease.clone() })
        }
        (
            EntityState::Starting { .. } | EntityState::Running { .. } | EntityState::Paused { .. },
            EntityEvent::StopRequested { lease },
        ) if matches(lease) => Some(EntityState::Stopping { lease: lease.clone() }),
        (
            EntityState::Starting { .. }
            | EntityState::Running { .. }
            | EntityState::Paused { .. }
            | EntityState::Stopping { .. },
            EntityEvent::RunEnded { lease },
        ) if matches(lease) => Some(EntityState::Stopped),
        (
            EntityState::Creating
            | EntityState::Stopped
            | EntityState::Failed { .. }
            | EntityState::Starting { .. }
            | EntityState::Running { .. }
            | EntityState::Paused { .. }
            | EntityState::Stopping { .. },
            EntityEvent::DeleteRequested,
        ) => Some(EntityState::Deleting),
        (
            EntityState::Creating
            | EntityState::Starting { .. }
            | EntityState::Running { .. }
            | EntityState::Paused { .. }
            | EntityState::Stopping { .. },
            EntityEvent::Failed { reason },
        ) => Some(EntityState::Failed { reason: reason.clone() }),
        (EntityState::Failed { .. }, EntityEvent::Recovered) => Some(EntityState::Stopped),
        _ => None,
    };
    next.ok_or_else(|| IllegalTransitionError {
        from: state.kind(),
        event: event.name(),
    })
}

/// Applies an event to a record through the transition table, keeping the current lease and the generation
/// consistent with the new state.
///
/// A start bumps the generation. A delete keeps the bound lease so whoever finishes it can end the run. On an
/// illegal transition the record is left unchanged.
pub fn apply_entity_event(record: &mut SandboxEntityRecord, event: &EntityEvent) -> Result<(), IllegalTransitionError> {
    let next = next_entity_state(&record.state, event)?;
    if matches!(event, EntityEvent::StartRequested { .. }) {
        record.generation = record.generation.saturating_add(1);
    }
    if !matches!(event, EntityEvent::DeleteRequested) {
        record.current_lease = next.lease().cloned();
    }
    record.state = next;
    Ok(())
}
