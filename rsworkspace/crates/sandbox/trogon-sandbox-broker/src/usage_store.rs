//! Per-lease usage counters shared by every broker replica.
//!
//! Counters live in their own store keyed by lease id instead of inside the
//! `LeaseRecord`: a dispatch-journalled lease must stay equal to its journal
//! entry for the revision-checked pause and extend writes, so a counter bumped
//! on every command cannot ride on it. The entry also outlives the active
//! lease record, which is how a released lease still reports final figures.

mod in_memory_lease_usage_store;
mod keys;
mod lease_usage_store;
mod nats_kv_lease_usage_store;
mod usage_entry;
mod usage_store_error;

pub use crate::constants::USAGE_RETENTION;
pub use in_memory_lease_usage_store::InMemoryLeaseUsageStore;
#[cfg(test)]
use keys::{legacy_kv_key, nats_kv_key, nats_kv_tenant_prefix};
pub use lease_usage_store::LeaseUsageStore;
pub use nats_kv_lease_usage_store::NatsKvLeaseUsageStore;
pub use usage_entry::UsageEntry;
pub use usage_store_error::UsageStoreError;

#[cfg(test)]
mod tests;
