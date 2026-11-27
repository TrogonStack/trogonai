//! Markers for lease releases whose follow-up work has not finished.
//!
//! A release removes the active-lease record first and only then refunds the
//! lease's quota, clears its drive claims and announces `lease_released`. A
//! broker that stops in between would lose all of that. The marker is written
//! before the record is removed, carries everything the follow-up needs, and is
//! dropped once the follow-up finished, so the reconciler can redo it from the
//! marker alone. Delete-by-id has its own, richer marker in
//! [`crate::pending_purge`]; this one serves every other release.

mod constants;
mod in_memory_pending_release_store;
mod nats_kv_pending_release_store;
mod pending_release_error;
mod pending_release_marker;
mod pending_release_store;

pub use constants::PENDING_RELEASE_MAX_AGE;
pub use in_memory_pending_release_store::InMemoryPendingReleaseStore;
pub use nats_kv_pending_release_store::NatsKvPendingReleaseStore;
pub use pending_release_error::PendingReleaseError;
pub use pending_release_marker::PendingRelease;
pub use pending_release_store::PendingReleaseStore;

#[cfg(test)]
mod tests;
