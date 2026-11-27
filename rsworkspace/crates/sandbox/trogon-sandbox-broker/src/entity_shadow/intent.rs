//! Desired state writers: record what a verb asks for on the entity before
//! the verb acts, so a broker that dies mid-operation leaves the intent behind
//! for the reconciler to finish.

use trogon_sandbox_contract::{EntityState, LeaseId, SandboxEntityId, TenantId};

use super::EntityShadow;
use crate::entity_store::EntityStoreError;
use trogon_sandbox_protocol::{EntityEvent, apply_entity_event};

/// What a verb wants the entity to become.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Intent {
    /// End the run: `Stopping`.
    Stop,
    /// Freeze the run: `Paused`.
    Pause,
    /// Thaw the run: back to `Running`.
    Resume,
    /// Remove the entity: `Deleting`.
    Delete,
}

/// What recording an [`Intent`] did.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Recorded {
    /// The entity moved to the desired state.
    Written,
    /// It was already there; nothing was written.
    Already,
    /// The entity is not in a state this intent applies to (for example it is
    /// bound to another lease). The verb proceeds on the legacy stores alone.
    NotApplicable,
}

/// The event that moves `state` toward `intent` for `lease`. `Ok(None)` when
/// it is already there, `Err(())` when the intent does not apply.
#[allow(clippy::result_unit_err)]
pub fn event_for(state: &EntityState, intent: Intent, lease: Option<&LeaseId>) -> Result<Option<EntityEvent>, ()> {
    let bound = lease.is_some() && state.lease() == lease;
    let lease = || lease.cloned().ok_or(());
    match (intent, state) {
        (Intent::Stop, EntityState::Stopping { .. }) if bound => Ok(None),
        (Intent::Stop, EntityState::Starting { .. } | EntityState::Running { .. } | EntityState::Paused { .. })
            if bound =>
        {
            Ok(Some(EntityEvent::StopRequested { lease: lease()? }))
        }
        (Intent::Pause, EntityState::Paused { .. }) if bound => Ok(None),
        (Intent::Pause, EntityState::Running { .. }) if bound => Ok(Some(EntityEvent::Paused { lease: lease()? })),
        (Intent::Resume, EntityState::Running { .. }) if bound => Ok(None),
        (Intent::Resume, EntityState::Paused { .. }) if bound => Ok(Some(EntityEvent::Resumed { lease: lease()? })),
        (Intent::Delete, EntityState::Deleting) => Ok(None),
        (
            Intent::Delete,
            EntityState::Creating
            | EntityState::Stopped
            | EntityState::Failed { .. }
            | EntityState::Starting { .. }
            | EntityState::Running { .. }
            | EntityState::Paused { .. }
            | EntityState::Stopping { .. },
        ) => Ok(Some(EntityEvent::DeleteRequested)),
        _ => Err(()),
    }
}

impl EntityShadow {
    /// Writes `intent` on the entity, compare-and-swap, only when it is not
    /// already recorded. `lease` is the lease the verb targets; a delete needs none. Writing nothing
    /// when nothing changes keeps the record revision stable, and the
    /// reconciler derives its operation ids from that revision.
    pub async fn record_intent(
        &self,
        tenant: &TenantId,
        id: &SandboxEntityId,
        lease: Option<&LeaseId>,
        intent: Intent,
    ) -> Result<Recorded, EntityStoreError> {
        let Some(found) = self.store.get(tenant, id).await? else {
            return Ok(Recorded::NotApplicable);
        };
        match event_for(&found.record.state, intent, lease) {
            Ok(None) => return Ok(Recorded::Already),
            Err(()) => return Ok(Recorded::NotApplicable),
            Ok(Some(_)) => {}
        }
        self.store
            .mutate(tenant, id, &|record| {
                match event_for(&record.state, intent, lease) {
                    // A concurrent writer got there first.
                    Ok(None) | Err(()) => Ok(()),
                    Ok(Some(event)) => apply_entity_event(&mut *record, &event).map_err(Into::into),
                }
            })
            .await?;
        Ok(Recorded::Written)
    }
}

#[cfg(test)]
mod tests;
