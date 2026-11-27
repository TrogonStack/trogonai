//! Digest each lease resolved when it started, kept so a lease can be traced
//! to its snapshot until the record is purged.

mod in_memory_resolved_digest_store;
mod nats_kv_resolved_digest_store;
mod resolved_digest_error;
mod resolved_digest_store;

pub use in_memory_resolved_digest_store::InMemoryResolvedDigestStore;
pub use nats_kv_resolved_digest_store::NatsKvResolvedDigestStore;
pub use resolved_digest_error::ResolvedDigestError;
pub use resolved_digest_store::ResolvedDigestStore;

#[cfg(test)]
mod tests;
