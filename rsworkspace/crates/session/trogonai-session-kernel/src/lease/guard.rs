use async_nats::jetstream::kv;
use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};
use trogonai_session_contracts::SessionId;

use super::operation::SessionMutatingOperation;
use crate::state::LeaseState;

/// Guard representing an acquired Session Lease for one mutating operation.
pub struct SessionLeaseGuard<L> {
    session_id: SessionId,
    lease: L,
    revision: u64,
    operation: SessionMutatingOperation,
    holder_id: String,
    state: LeaseState,
}

impl<L> SessionLeaseGuard<L>
where
    L: TryAcquireLease<Error = kv::CreateError>
        + RenewLease<Error = kv::UpdateError>
        + ReleaseLease<Error = kv::DeleteError>
        + Clone
        + Send
        + Sync
        + 'static,
{
    pub fn new(
        session_id: SessionId,
        lease: L,
        revision: u64,
        operation: SessionMutatingOperation,
        holder_id: String,
        state: LeaseState,
    ) -> Self {
        Self {
            session_id,
            lease,
            revision,
            operation,
            holder_id,
            state,
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn operation(&self) -> SessionMutatingOperation {
        self.operation
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn holder_id(&self) -> &str {
        &self.holder_id
    }

    pub fn state(&self) -> LeaseState {
        self.state
    }

    pub fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    pub fn set_state(&mut self, state: LeaseState) {
        self.state = state;
    }

    pub fn lease(&self) -> &L {
        &self.lease
    }
}
