use std::sync::{Arc, Mutex, PoisonError};

use trogon_nats::RequestClient;
use trogon_std::time::EpochClock;

use super::constants::MAX_CACHED_RELEASES;
use super::{
    InvalidationReader, InvalidationStartError, InvalidationStreamSource, ReleaseCache, ReleaseCacheKey,
    ReleaseCacheMode, ReleaseClient, ReleaseError, ReleaseRequest, ReleasedValue,
};

/// A [`ReleaseClient`] that holds what it was released and drops it when the
/// secrets service says the credential changed.
///
/// It holds at most [`MAX_CACHED_RELEASES`] copies, each until the earlier of
/// its `valid_until` and [`crate::RELEASE_HOLD_BOUND`] after it arrived,
/// pulled ahead by up to [`crate::RELEASE_CACHE_JITTER`] (ADR#0049). A failed
/// release is never held.
///
/// It reads `SECRETS_INVALIDATION` through an ordered consumer of its own and
/// drops every copy of a credential a notice names, for every host and
/// location and whatever the reason; a notice whose subject it cannot read
/// drops every copy. While that reader has lost its place nothing is held
/// and every release is asked for again. Clones share the cache and the
/// reader, which stops when the last clone is dropped.
#[derive(Clone)]
pub struct CachedReleaseClient<N, C> {
    client: ReleaseClient<N, C>,
    cache: Arc<Mutex<ReleaseCache>>,
    _reader: Arc<InvalidationReader>,
}

impl<N, C> CachedReleaseClient<N, C>
where
    N: RequestClient,
    N::RequestError: 'static,
    C: EpochClock,
{
    /// Starts reading invalidations from `jetstream`, and refuses to start
    /// when it cannot, since a copy it never hears revoked would keep serving.
    pub async fn start<J>(client: ReleaseClient<N, C>, jetstream: J) -> Result<Self, InvalidationStartError>
    where
        J: InvalidationStreamSource,
    {
        let cache = Arc::new(Mutex::new(ReleaseCache::with_capacity(MAX_CACHED_RELEASES)));
        let reader = InvalidationReader::start(jetstream, client.clock().clone(), Arc::clone(&cache)).await?;
        Ok(Self {
            client,
            cache,
            _reader: Arc::new(reader),
        })
    }

    /// The copy held for `release`, or what the secrets service releases for
    /// it now.
    pub async fn release(&self, release: &ReleaseRequest) -> Result<ReleasedValue, ReleaseError> {
        let key = ReleaseCacheKey::of(release);
        let ticket = {
            let mut cache = self.cache();
            if let Some(held) = cache.get(&key, self.client.clock().system_time()) {
                return Ok(held);
            }
            cache.ticket()
        };
        let released = self.client.release(release).await?;
        self.cache()
            .put(ticket, key, released.clone(), self.client.clock().system_time());
        Ok(released)
    }

    /// Whether releases are held right now, or all asked for again because
    /// the invalidation reader lost its place.
    pub fn mode(&self) -> ReleaseCacheMode {
        self.cache().mode()
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, ReleaseCache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
