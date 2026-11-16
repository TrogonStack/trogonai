//! What the release cache and its invalidation reader record.

mod metrics;
#[cfg(test)]
mod recorded_metrics;

pub(crate) use metrics::ReleaseCacheMetrics;
#[cfg(test)]
pub(crate) use recorded_metrics::RecordedMetrics;
