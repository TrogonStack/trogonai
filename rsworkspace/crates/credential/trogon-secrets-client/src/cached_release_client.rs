use std::sync::{Arc, Mutex, PoisonError};

use opentelemetry::global;
use opentelemetry::metrics::Meter;
use trogon_nats::RequestClient;
use trogon_std::time::{EpochClock, GetElapsed};

use super::constants::{MAX_CACHED_RELEASES, METER_NAME};
use super::telemetry::ReleaseCacheMetrics;
use super::{
    InvalidationReader, InvalidationStartError, InvalidationStreamSource, ReleaseCache, ReleaseCacheKey,
    ReleaseCacheMode, ReleaseClient, ReleaseError, ReleaseHold, ReleaseRequest, ReleasedValue,
};

/// A [`ReleaseClient`] that holds what it was released and drops it when the
/// secrets service says the credential changed.
///
/// It holds at most [`MAX_CACHED_RELEASES`] copies, each until the earlier of
/// its `valid_until` and [`crate::RELEASE_HOLD_BOUND`] after it arrived,
/// pulled ahead by up to [`crate::RELEASE_CACHE_JITTER`] (ADR#0049). The
/// clock bounds that hold twice: on the wall, against `valid_until`, and on
/// its monotonic side, so a wall clock stepped back never stretches a hold.
/// A failed release is never held.
///
/// It reads `SECRETS_INVALIDATION` through an ordered consumer of its own and
/// drops every copy of a credential a notice names, for every host and
/// location and whatever the reason; a notice whose subject it cannot read
/// drops every copy. A release still on its way when such a notice arrives is
/// neither held nor returned, and is asked for once more. While that reader
/// has lost its place, or has not yet caught up with the notices its consumer
/// replays on starting and on every return to reading, nothing is held and
/// every release is asked for again. Clones share the cache and the reader,
/// which stops when the last clone is dropped.
///
/// It counts, on the `trogon-secrets-client` meter, each time its reader
/// loses its place and each release it serves without holding because the
/// cache is full, and warns of each loss of place and, at most once every
/// minute, of a full cache.
#[derive(Clone)]
pub struct CachedReleaseClient<N, C: GetElapsed> {
    client: ReleaseClient<N, C>,
    cache: Arc<Mutex<ReleaseCache<C>>>,
    metrics: ReleaseCacheMetrics,
    _reader: Arc<InvalidationReader>,
}

impl<N, C> CachedReleaseClient<N, C>
where
    N: RequestClient,
    N::RequestError: 'static,
    C: EpochClock + GetElapsed,
    C::Instant: Send,
{
    /// Starts reading invalidations from `jetstream`, and refuses to start
    /// when it cannot, since a copy it never hears revoked would keep serving.
    ///
    /// It returns once the reader has caught up with the notices already in
    /// the stream, or a quiet look finds it owed nothing more, so the replay
    /// is over before the first release, or once the reader has lost its
    /// place before that, when it serves
    /// [`ReleaseCacheMode::Uncached`] until it reads again.
    pub async fn start<J>(client: ReleaseClient<N, C>, jetstream: J) -> Result<Self, InvalidationStartError>
    where
        J: InvalidationStreamSource,
    {
        Self::start_with(client, jetstream, MAX_CACHED_RELEASES, &global::meter(METER_NAME)).await
    }

    /// Starts as [`CachedReleaseClient::start`] does, holding at most
    /// `capacity` releases and counting on `meter`.
    pub(crate) async fn start_with<J>(
        client: ReleaseClient<N, C>,
        jetstream: J,
        capacity: usize,
        meter: &Meter,
    ) -> Result<Self, InvalidationStartError>
    where
        J: InvalidationStreamSource,
    {
        let metrics = ReleaseCacheMetrics::new(meter);
        let cache = Arc::new(Mutex::new(ReleaseCache::with_capacity(
            capacity,
            client.clock().clone(),
        )));
        let reader =
            InvalidationReader::start(jetstream, client.clock().clone(), Arc::clone(&cache), metrics.clone()).await?;
        Ok(Self {
            client,
            cache,
            metrics,
            _reader: Arc::new(reader),
        })
    }

    /// The copy held for `release`, or what the secrets service releases for
    /// it now.
    ///
    /// A release that was on its way when an invalidation covered its
    /// credential, or when the reader lost or regained its place, may carry
    /// the revoked value, so it is dropped and asked for once more. When that
    /// one is overtaken too, the call fails with
    /// [`ReleaseError::Invalidated`] and returns no value.
    pub async fn release(&self, release: &ReleaseRequest) -> Result<ReleasedValue, ReleaseError> {
        let key = ReleaseCacheKey::of(release);
        if let Some(released) = self.held_or_fetched(release, &key).await? {
            return Ok(released);
        }
        self.held_or_fetched(release, &key)
            .await?
            .ok_or(ReleaseError::Invalidated)
    }

    /// The copy held under `key`, or a release fetched now, or nothing when
    /// that release was overtaken on its way.
    async fn held_or_fetched(
        &self,
        release: &ReleaseRequest,
        key: &ReleaseCacheKey,
    ) -> Result<Option<ReleasedValue>, ReleaseError> {
        let ticket = {
            let mut cache = self.cache();
            if let Some(held) = cache.get(key, self.client.clock().system_time()) {
                return Ok(Some(held));
            }
            cache.ticket()
        };
        let released = self.client.release(release).await?;
        let mut cache = self.cache();
        if !cache.is_current(ticket, key) {
            return Ok(None);
        }
        let now = self.client.clock().system_time();
        if cache.put(ticket, key.clone(), released.clone(), now) == ReleaseHold::Full {
            self.metrics.record_capacity_skip();
            if let Some(turned_away) = cache.capacity_warning() {
                let capacity = cache.capacity();
                tracing::warn!(
                    capacity,
                    turned_away,
                    "release cache is full; releases are served without being held"
                );
            }
        }
        Ok(Some(released))
    }

    /// Whether releases are held right now, or all asked for again because
    /// the invalidation reader lost its place and has not caught up since.
    pub fn mode(&self) -> ReleaseCacheMode {
        self.cache().mode()
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, ReleaseCache<C>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
