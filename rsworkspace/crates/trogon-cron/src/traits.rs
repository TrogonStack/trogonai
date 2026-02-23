use bytes::Bytes;
use std::future::Future;

/// Publish a tick notification to a NATS subject with trace headers.
///
/// One trait, one operation — implement this to replace the publish step in tests.
pub trait TickPublisher: Send + Sync + Clone + 'static {
    type Error: std::error::Error + Send + Sync;

    fn publish_tick(
        &self,
        subject: String,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Acquire, renew, or release a distributed leader lock via NATS KV.
///
/// Three operations, all part of the same lock lifecycle — kept in one trait
/// intentionally since they are semantically inseparable.
pub trait LeaderLock: Send + Sync + Clone + 'static {
    type Error: std::error::Error + Send + Sync;

    /// Atomically create the lock. Returns revision on success, Err if already held.
    fn try_acquire(
        &self,
        value: Bytes,
    ) -> impl Future<Output = Result<u64, Self::Error>> + Send;

    /// CAS-update the lock to renew the TTL. Returns new revision, Err on mismatch.
    fn renew(
        &self,
        value: Bytes,
        revision: u64,
    ) -> impl Future<Output = Result<u64, Self::Error>> + Send;

    /// Delete the lock on graceful shutdown so the next leader is elected immediately.
    fn release(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
