use std::sync::{Arc, Mutex, PoisonError};

use super::{ReleaseCacheKey, ReleaseFlights};

/// Takes a call off the board once dropped: when it lands, or when its task
/// ends any other way, so a miss never waits on a call that is over.
pub(crate) struct ReleaseFlightLanding {
    flights: Arc<Mutex<ReleaseFlights>>,
    key: ReleaseCacheKey,
    id: u64,
}

impl ReleaseFlightLanding {
    pub(crate) const fn new(flights: Arc<Mutex<ReleaseFlights>>, key: ReleaseCacheKey, id: u64) -> Self {
        Self { flights, key, id }
    }
}

impl Drop for ReleaseFlightLanding {
    fn drop(&mut self) {
        self.flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .land(&self.key, self.id);
    }
}

#[cfg(test)]
mod tests;
