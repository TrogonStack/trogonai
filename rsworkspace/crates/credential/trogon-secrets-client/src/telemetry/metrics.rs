use std::fmt;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter};
use trogon_semconv::{attribute, metric};

use crate::InvalidationLoss;

/// The counters the release cache and its invalidation reader record on.
#[derive(Clone)]
pub(crate) struct ReleaseCacheMetrics {
    position_losses: Counter<u64>,
    capacity_skips: Counter<u64>,
}

impl ReleaseCacheMetrics {
    pub(crate) fn new(meter: &Meter) -> Self {
        Self {
            position_losses: metric::build_secrets_client_invalidation_position_losses(meter),
            capacity_skips: metric::build_secrets_client_release_cache_capacity_skips(meter),
        }
    }

    /// Counts the invalidation reader losing its place, under why it lost it.
    pub(crate) fn record_position_loss(&self, loss: &InvalidationLoss) {
        self.position_losses.add(
            1,
            &[KeyValue::new(
                attribute::SECRETS_CLIENT_INVALIDATION_LOSS_REASON,
                loss.reason().as_str(),
            )],
        );
    }

    /// Counts a release served without being held because the cache was full.
    pub(crate) fn record_capacity_skip(&self) {
        self.capacity_skips.add(1, &[]);
    }
}

impl fmt::Debug for ReleaseCacheMetrics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ReleaseCacheMetrics").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
