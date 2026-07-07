//! Pluggable state persistence for pending token requests and missions, per
//! "Deferred Responses" (#deferred-responses) and "Mission Management"
//! (#mission-management).
//!
//! [`InMemoryStore`] is the reference implementation used by tests and
//! single-process deployments. A durable store (e.g. backed by a database)
//! implements the same [`PersonStateStore`] trait; nothing in
//! [`crate::server::PersonServer`] assumes in-process state.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::mission::{Mission, MissionId};
use crate::pending::{PendingId, PendingRequest};

/// Failures reading or writing state. `InMemoryStore` never produces these;
/// the variant exists for durable implementations (e.g. connection errors).
#[derive(Debug, thiserror::Error)]
#[error("state store error: {0}")]
pub struct StoreError(pub String);

/// Persistence seam for pending token-endpoint requests and missions.
/// Implementations MUST make `insert_pending`/`update_pending` atomic per
/// `PendingId` -- "Concurrent Requests" (#concurrent-requests) requires the
/// PS to correlate repeated requests for the same agent+resource onto one
/// pending entry rather than creating duplicates.
#[async_trait]
pub trait PersonStateStore: Send + Sync {
    async fn insert_pending(&self, pending: PendingRequest) -> Result<(), StoreError>;
    async fn get_pending(&self, id: &PendingId) -> Result<Option<PendingRequest>, StoreError>;
    async fn update_pending(&self, pending: PendingRequest) -> Result<(), StoreError>;
    async fn remove_pending(&self, id: &PendingId) -> Result<(), StoreError>;

    /// Finds an existing pending request for the same `(agent, resource
    /// issuer, resource jti)` tuple, per "Concurrent Requests": repeated
    /// token requests for the same outstanding grant correlate onto the same
    /// pending flow instead of spawning parallel interactions.
    async fn find_pending_by_correlation(&self, key: &str) -> Result<Option<PendingId>, StoreError>;

    /// Atomically inserts `pending` unless a non-terminal request with the
    /// same correlation key already exists, returning the existing id when
    /// one does. Implementations MUST make the check and the insert a single
    /// atomic step -- a separate find-then-insert lets two concurrent
    /// requests for the same agent+resource spawn parallel flows, breaking
    /// "Concurrent Requests".
    async fn insert_pending_unless_correlated(&self, pending: PendingRequest) -> Result<Option<PendingId>, StoreError>;

    async fn insert_mission(&self, mission: Mission) -> Result<(), StoreError>;
    async fn get_mission(&self, id: &MissionId) -> Result<Option<Mission>, StoreError>;
    async fn update_mission(&self, mission: Mission) -> Result<(), StoreError>;
}

/// In-memory reference implementation. Not durable across process restarts;
/// suitable for tests and single-node deployments willing to lose in-flight
/// interactions on crash.
#[derive(Default)]
pub struct InMemoryStore {
    pending: Mutex<HashMap<PendingId, PendingRequest>>,
    correlation: Mutex<HashMap<String, PendingId>>,
    missions: Mutex<HashMap<MissionId, Mission>>,
}

impl InMemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PersonStateStore for InMemoryStore {
    async fn insert_pending(&self, pending: PendingRequest) -> Result<(), StoreError> {
        let key = pending.correlation_key.clone();
        let id = pending.id.clone();
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), pending);
        self.correlation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, id);
        Ok(())
    }

    async fn get_pending(&self, id: &PendingId) -> Result<Option<PendingRequest>, StoreError> {
        Ok(self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned())
    }

    async fn update_pending(&self, pending: PendingRequest) -> Result<(), StoreError> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(pending.id.clone(), pending);
        Ok(())
    }

    async fn remove_pending(&self, id: &PendingId) -> Result<(), StoreError> {
        let removed = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        if let Some(p) = removed {
            self.correlation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&p.correlation_key);
        }
        Ok(())
    }

    async fn find_pending_by_correlation(&self, key: &str) -> Result<Option<PendingId>, StoreError> {
        Ok(self
            .correlation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned())
    }

    async fn insert_pending_unless_correlated(&self, pending: PendingRequest) -> Result<Option<PendingId>, StoreError> {
        // Same lock order as insert_pending (pending, then correlation) so
        // the two paths cannot deadlock against each other.
        let mut entries = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut correlation = self
            .correlation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing_id) = correlation.get(&pending.correlation_key)
            && let Some(existing) = entries.get(existing_id)
            && !existing.is_terminal()
        {
            return Ok(Some(existing_id.clone()));
        }
        let key = pending.correlation_key.clone();
        let id = pending.id.clone();
        entries.insert(id.clone(), pending);
        correlation.insert(key, id);
        Ok(None)
    }

    async fn insert_mission(&self, mission: Mission) -> Result<(), StoreError> {
        self.missions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(mission.id.clone(), mission);
        Ok(())
    }

    async fn get_mission(&self, id: &MissionId) -> Result<Option<Mission>, StoreError> {
        Ok(self
            .missions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned())
    }

    async fn update_mission(&self, mission: Mission) -> Result<(), StoreError> {
        self.missions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(mission.id.clone(), mission);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
