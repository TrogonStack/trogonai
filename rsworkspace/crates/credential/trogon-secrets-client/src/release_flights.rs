use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use futures::FutureExt;

use super::release_flight::Landed;
use super::{ReleaseCacheKey, ReleaseError, ReleaseFlight};

/// The calls to the secrets service on their way, at most one per cache key.
#[derive(Default)]
pub(crate) struct ReleaseFlights {
    flights: BTreeMap<ReleaseCacheKey, ReleaseFlight>,
    boarded: u64,
}

impl ReleaseFlights {
    /// The call on its way for `key`, if there is one.
    pub(crate) fn get(&self, key: &ReleaseCacheKey) -> Option<&ReleaseFlight> {
        self.flights.get(key)
    }

    /// Spawns `fly` as the call for `key`, asked at `ticket`, in place of any
    /// call already there.
    ///
    /// `fly` is handed the call's id, to land it with. A task that ends
    /// without landing, by panicking or with its runtime, fails whoever
    /// waits on it.
    pub(crate) fn board<F>(&mut self, key: ReleaseCacheKey, ticket: u64, fly: impl FnOnce(u64) -> F) -> ReleaseFlight
    where
        F: Future<Output = Landed> + Send + 'static,
    {
        self.boarded += 1;
        let id = self.boarded;
        let landed = tokio::spawn(fly(id))
            .map(|joined| joined.unwrap_or_else(|ended| Err(ReleaseError::Request(Arc::new(ended)))))
            .boxed()
            .shared();
        let flight = ReleaseFlight::new(id, ticket, landed);
        self.flights.insert(key, flight.clone());
        flight
    }

    /// Takes call `id` off `key`, unless a later call took its place.
    pub(crate) fn land(&mut self, key: &ReleaseCacheKey, id: u64) {
        if self.flights.get(key).is_some_and(|flight| flight.id() == id) {
            self.flights.remove(key);
        }
    }
}

#[cfg(test)]
mod tests;
