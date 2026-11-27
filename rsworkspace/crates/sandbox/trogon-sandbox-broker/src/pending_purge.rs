//! Markers for delete-by-id requests whose purge has not finished.
//!
//! Deleting a lease by id first releases the lease, which removes its
//! active-lease record, and only then removes what would let the lease be
//! restored (its snapshot tag above all). A marker written before the release
//! keeps the lease's tenant and name reachable after the record is gone, so a
//! retried delete can finish the purge instead of answering not found, and the
//! reconciler can finish it when nobody retries.

mod constants;
mod in_memory_pending_purge_store;
mod nats_kv_pending_purge_store;
mod pending_purge_error;
mod pending_purge_store;
mod pending_refund;
mod purge_marker;

pub use constants::PENDING_PURGE_MAX_AGE;
pub use in_memory_pending_purge_store::InMemoryPendingPurgeStore;
pub use nats_kv_pending_purge_store::NatsKvPendingPurgeStore;
pub use pending_purge_error::PendingPurgeError;
pub use pending_purge_store::PendingPurgeStore;
pub use pending_refund::PendingRefund;
pub use purge_marker::PendingPurge;

#[cfg(test)]
mod tests;
